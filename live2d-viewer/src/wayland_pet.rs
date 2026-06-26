use std::path::PathBuf;
use std::sync::mpsc;

use serde::Deserialize;
use smithay_client_toolkit::reexports::client::globals::{registry_queue_init, GlobalListContents};
use smithay_client_toolkit::reexports::client::protocol::wl_compositor;
use smithay_client_toolkit::reexports::client::protocol::wl_pointer;
use smithay_client_toolkit::reexports::client::protocol::wl_region;
use smithay_client_toolkit::reexports::client::protocol::wl_seat;
use smithay_client_toolkit::reexports::client::protocol::{wl_registry, wl_surface};
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};
use smithay_client_toolkit::reexports::protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};

use std::ffi::c_void;
use std::num::NonZeroU32;

use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::ContextAttributesBuilder;
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, WindowSurface};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};

/// Main thread → Pet thread commands
pub enum PetCommand {
    Enter {
        model_dir: PathBuf,
        model_format: crate::app::ModelFormat,
        /// Initial click-through state (input passthrough) from main window
        click_through: bool,
    },
    /// Toggle click-through state at runtime
    SetClickThrough(bool),
    /// Overwrite model parameter values (synced from main thread each frame)
    SetParameters {
        values: Vec<f32>,
        part_opacities: Vec<f32>,
    },
    Exit,
}

/// Pet thread → Main thread events
pub enum PetEvent {
    Configured {
        width: u32,
        height: u32,
    },
    /// Raw tap (click without drag) with coordinates + viewport + camera.
    /// Main thread does the full hit test (V2/V3 dispatch, drawable hit, motion).
    Tap {
        x: f64,
        y: f64,
        w: f32,
        h: f32,
        cam_scale_x: f32,
        cam_scale_y: f32,
        cam_translate_x: f32,
        cam_translate_y: f32,
    },
    /// Cursor moved on the overlay surface (main thread uses for look-at tracking).
    CursorMoved {
        x: f64,
        y: f64,
        w: f32,
        h: f32,
    },
    Error(String),
    Exited,
}

/// The model variant loaded in the pet thread.
#[allow(clippy::large_enum_variant)]
enum PetModel {
    V3 {
        _moc: live2d_core::Moc,
        model: live2d_core::Model<'static>,
        renderer: crate::renderer::Live2dRenderer,
        camera: crate::camera::Camera,
        look: crate::motion::look::Look,
        param_lookup: std::collections::HashMap<String, usize>,
    },
    V2 {
        v2_model: live2d_v2_core::Model,
        v2_vao: glow::NativeVertexArray,
        last_size: (i32, i32),
        hit_areas: Vec<crate::model_loader::HitArea>,
    },
}

// ---------------------------------------------------------------------------
// Pointer / drag state
// ---------------------------------------------------------------------------

struct PointerState {
    pointer_x: f64,
    pointer_y: f64,
    dragging: bool,
    drag_start_x: f64,
    drag_start_y: f64,
    had_motion: bool,
    /// Current margin (persistent across frames, updated each frame end)
    margin_right: i32,
    margin_bottom: i32,
    /// Margin snapshot taken at the START of each frame, before dispatch.
    /// All Motion events within this frame compute new_mr relative to this,
    /// avoiding both intra-frame double-subtraction and inter-frame drift
    /// (surface_x resets after each frame's commits take effect).
    frame_margin_right: i32,
    frame_margin_bottom: i32,
    pending_click: Option<(f64, f64)>,
    /// Last cursor position sent to main thread (for throttling look-at updates)
    last_cursor_x: f64,
    last_cursor_y: f64,
}

impl PointerState {
    fn new() -> Self {
        Self {
            pointer_x: 0.0,
            pointer_y: 0.0,
            dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            had_motion: false,
            margin_right: 20,
            margin_bottom: 20,
            frame_margin_right: 20,
            frame_margin_bottom: 20,
            pending_click: None,
            last_cursor_x: f64::NEG_INFINITY,
            last_cursor_y: f64::NEG_INFINITY,
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct PetState {
    configured_size: Option<(u32, u32)>,
    compositor: wl_compositor::WlCompositor,
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    ptr: PointerState,
    event_tx: mpsc::Sender<PetEvent>,
}

// ---------------------------------------------------------------------------
// Dispatch implementations for raw Wayland protocol types
// ---------------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PetState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for PetState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_compositor::WlCompositor,
        _event: wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("wl_compositor has no events")
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for PetState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for PetState {
    fn event(
        _state: &mut Self,
        _proxy: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        unreachable!("zwlr_layer_shell_v1 has no events")
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for PetState {
    fn event(
        state: &mut Self,
        _proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                if width > 0 && height > 0 {
                    state.configured_size = Some((width, height));
                }
                state.layer_surface.ack_configure(serial);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                log::info!("[pet/wayland] layer surface closed by compositor");
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_region::WlRegion, ()> for PetState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for PetState {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let cap = match capabilities {
                    smithay_client_toolkit::reexports::client::WEnum::Value(c) => c,
                    _ => return,
                };
                if cap.contains(wl_seat::Capability::Pointer) {
                    let pointer = seat.get_pointer(&qh, ());
                    state.pointer = Some(pointer);
                    log::info!("[pet/wayland] got wl_pointer");
                }
            }
            wl_seat::Event::Name { name } => {
                log::debug!("[pet/wayland] seat name: {name}");
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for PetState {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                state.ptr.pointer_x = surface_x;
                state.ptr.pointer_y = surface_y;
            }
            wl_pointer::Event::Leave { .. } => {
                state.ptr.dragging = false;
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if state.ptr.dragging {
                    let offset_x = surface_x - state.ptr.drag_start_x;
                    let offset_y = surface_y - state.ptr.drag_start_y;

                    if offset_x.abs() + offset_y.abs() > 3.0 {
                        state.ptr.had_motion = true;
                    }
                    // Compute from frame-start margin snapshot.
                    // frame_margin_right is updated at the start of each frame BEFORE
                    // dispatch, so all Motion events in this batch share the same base.
                    // This avoids:
                    //   - intra-frame: multiple Motion events corrupting margin updates
                    //   - inter-frame: surface_x reset after commit → offset_x under-reports
                    //     total drag movement (surface_x is surface-local, not absolute)
                    let new_mr = (state.ptr.frame_margin_right as f64 - offset_x).round() as i32;
                    let new_mb = (state.ptr.frame_margin_bottom as f64 - offset_y).round() as i32;
                    state.ptr.margin_right = new_mr.max(0);
                    state.ptr.margin_bottom = new_mb.max(0);
                    state.layer_surface.set_margin(
                        0,
                        state.ptr.margin_right,
                        state.ptr.margin_bottom,
                        0,
                    );
                    state.surface.commit();
                } else {
                    state.ptr.pointer_x = surface_x;
                    state.ptr.pointer_y = surface_y;
                    // Forward cursor to main thread for look-at tracking (throttled).
                    // The main thread updates its look target, advances the look
                    // system, and the modified params flow back via SetParameters.
                    let dist = (surface_x - state.ptr.last_cursor_x).abs()
                        + (surface_y - state.ptr.last_cursor_y).abs();
                    if dist > 2.0 {
                        state.ptr.last_cursor_x = surface_x;
                        state.ptr.last_cursor_y = surface_y;
                        if let Some(size) = state.configured_size {
                            let _ = state.event_tx.send(PetEvent::CursorMoved {
                                x: surface_x,
                                y: surface_y,
                                w: size.0 as f32,
                                h: size.1 as f32,
                            });
                        }
                    }
                }
            }
            wl_pointer::Event::Button {
                button,
                state: btn_state,
                ..
            } => {
                const BTN_LEFT: u32 = 0x110;
                if button == BTN_LEFT {
                    let btn = match btn_state {
                        smithay_client_toolkit::reexports::client::WEnum::Value(b) => b,
                        _ => return,
                    };
                    match btn {
                        wl_pointer::ButtonState::Pressed => {
                            state.ptr.dragging = true;
                            state.ptr.drag_start_x = state.ptr.pointer_x;
                            state.ptr.drag_start_y = state.ptr.pointer_y;
                            state.ptr.had_motion = false;
                        }
                        wl_pointer::ButtonState::Released => {
                            let was_click = state.ptr.dragging && !state.ptr.had_motion;
                            state.ptr.dragging = false;
                            if was_click {
                                state.ptr.pending_click =
                                    Some((state.ptr.pointer_x, state.ptr.pointer_y));
                            }
                        }
                        _ => {}
                    }
                }
            }
            wl_pointer::Event::Frame => {}
            wl_pointer::Event::Axis { .. } => {}
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Surface setup
// ---------------------------------------------------------------------------

fn setup_pet_surface(
    model_dir: &PathBuf,
    model_format: crate::app::ModelFormat,
    click_through: bool,
    cmd_rx: &mpsc::Receiver<PetCommand>,
    event_tx: &mpsc::Sender<PetEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();
    let (globals, _) = registry_queue_init::<PetState>(&conn)?;

    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ())?;
    let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ())?;
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=9, ())?;

    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "live2d-pet".to_string(),
        &qh,
        (),
    );
    layer_surface.set_size(400, 500);
    layer_surface
        .set_anchor(zwlr_layer_surface_v1::Anchor::Bottom | zwlr_layer_surface_v1::Anchor::Right);
    layer_surface.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    layer_surface.set_margin(0, 20, 20, 0);

    // Apply initial click-through state (empty input region → passthrough)
    if click_through {
        let empty_region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&empty_region));
    }
    surface.commit();

    // Clone surface for GL context (PetState takes ownership)
    let surface_for_gl = surface.clone();
    let mut state = PetState {
        configured_size: None,
        compositor,
        surface,
        layer_surface,
        seat: Some(seat),
        pointer: None,
        ptr: PointerState::new(),
        event_tx: event_tx.clone(),
    };

    event_queue.roundtrip(&mut state)?;

    if let Some((w, h)) = state.configured_size {
        let _ = event_tx.send(PetEvent::Configured {
            width: w,
            height: h,
        });
    }

    // === GL context creation ===
    let display_ptr = conn.backend().display_ptr() as *mut c_void;
    let mut raw_display_handle = WaylandDisplayHandle::empty();
    raw_display_handle.display = display_ptr;
    let raw_display = RawDisplayHandle::Wayland(raw_display_handle);

    let gl_display = unsafe { Display::new(raw_display, DisplayApiPreference::Egl)? };

    let template = ConfigTemplateBuilder::new().with_alpha_size(8).build();
    let gl_config = unsafe { gl_display.find_configs(template)? }
        .next()
        .ok_or_else(|| anyhow::anyhow!("no suitable GL config"))?;

    let surface_ptr = surface_for_gl.id().as_ptr() as *mut c_void;
    let mut raw_window_handle = WaylandWindowHandle::empty();
    raw_window_handle.surface = surface_ptr;
    let raw_window = RawWindowHandle::Wayland(raw_window_handle);

    let context_attrs = ContextAttributesBuilder::new().build(Some(raw_window));
    let not_current = unsafe { gl_display.create_context(&gl_config, &context_attrs)? };

    let (init_w, init_h) = state.configured_size.unwrap_or((400, 500));
    let phys_w =
        NonZeroU32::new(init_w.max(1)).ok_or_else(|| std::io::Error::other("zero width"))?;
    let phys_h =
        NonZeroU32::new(init_h.max(1)).ok_or_else(|| std::io::Error::other("zero height"))?;
    let surf_attrs =
        SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_window, phys_w, phys_h);
    let egl_surface = unsafe { gl_display.create_window_surface(&gl_config, &surf_attrs)? };

    let gl_context = not_current.make_current(&egl_surface)?;

    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            let c_str = std::ffi::CString::new(s).expect("gl proc name");
            gl_display.get_proc_address(&c_str) as *const _
        })
    };

    // Test clear + swap
    unsafe {
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
    }
    egl_surface.swap_buffers(&gl_context)?;

    // Initialize V2 glad loader (harmless if no V2 features used)
    if live2d_v2_core::gl_init() == 0 {
        log::warn!("[pet/wayland] V2 glInit returned 0 — V2 rendering may not work");
    }

    // Create a VAO for V2 rendering (V2 uses core-profile-incompatible no-VAO GL 2.1 pattern)
    let v2_vao = unsafe { gl.create_vertex_array().expect("create V2 VAO") };

    // === Model loading (branch by format) ===
    let init_size = state.configured_size.unwrap_or((400, 500));

    let pet_model = match model_format {
        crate::app::ModelFormat::V3 => {
            // Find model3.json by scanning directory (supports any filename)
            let model3_path = crate::model_loader::find_model3_json(model_dir)
                .map_err(|e| anyhow::anyhow!("find model3.json: {e}"))?;
            // Base dir for resolving relative paths (moc, textures)
            let base_dir = model3_path.parent().unwrap_or(model_dir).to_path_buf();
            let model3_json = crate::model_loader::Model3Json::from_file(&model3_path)
                .map_err(|e| anyhow::anyhow!("parse model3.json: {e}"))?;

            let moc_path = base_dir.join(&model3_json.file_references.moc);
            let moc_data =
                std::fs::read(&moc_path).map_err(|e| anyhow::anyhow!("read moc3: {e}"))?;

            let moc = live2d_core::Moc::revive(&moc_data)
                .map_err(|e| anyhow::anyhow!("Moc::revive: {e}"))?;
            let moc_ptr: *const live2d_core::Moc = &moc as *const live2d_core::Moc;
            let mut model: live2d_core::Model<'static> = unsafe {
                std::mem::transmute(
                    live2d_core::Model::initialize(&*moc_ptr)
                        .map_err(|e| anyhow::anyhow!("Model::initialize: {e}"))?,
                )
            };

            // Load textures (paths relative to base_dir)
            let mut textures = Vec::new();
            for tex_path in &model3_json.file_references.textures {
                let full_path = base_dir.join(tex_path);
                if let Ok(data) = std::fs::read(&full_path) {
                    if let Ok(tex) = unsafe { crate::texture::load_texture(&gl, &data) } {
                        textures.push(tex);
                    } else {
                        log::warn!("[pet/wayland] failed to load texture: {full_path:?}");
                    }
                } else {
                    log::warn!("[pet/wayland] texture file not found: {full_path:?}");
                }
            }

            // Initialize renderer
            let mut renderer = unsafe {
                crate::renderer::Live2dRenderer::new(&gl)
                    .map_err(|e| anyhow::anyhow!("renderer: {e}"))?
            };
            renderer.textures = textures;
            model.update();

            // Camera sized to the pet window
            let mut camera = crate::camera::Camera::new();
            let canvas_info = model.canvas_info();
            camera.fit_to_canvas(
                canvas_info.size_in_pixels.X,
                canvas_info.size_in_pixels.Y,
                canvas_info.pixels_per_unit,
                init_size.0 as f32,
                init_size.1 as f32,
            );

            log::info!(
                "[pet/wayland] V3 model loaded, canvas=({:.0},{:.0}), configured size: {:?}",
                canvas_info.size_in_pixels.X,
                canvas_info.size_in_pixels.Y,
                state.configured_size
            );

            let param_lookup: std::collections::HashMap<String, usize> = model
                .parameters()
                .ids()
                .iter()
                .enumerate()
                .map(|(i, id)| (id.to_string_lossy().into_owned(), i))
                .collect();

            PetModel::V3 {
                _moc: moc,
                model,
                renderer,
                camera,
                look: crate::motion::look::Look::new(),
                param_lookup,
            }
        }
        crate::app::ModelFormat::V2 => {
            // Try common V2 model JSON filenames
            let model_json_path = {
                let mj = model_dir.join("model.json");
                if mj.exists() {
                    mj
                } else {
                    let m0 = model_dir.join("model0.json");
                    if m0.exists() {
                        m0
                    } else {
                        return Err(anyhow::anyhow!(
                            "no V2 model JSON (model.json/model0.json) found in {:?}",
                            model_dir
                        )
                        .into());
                    }
                }
            };

            let mut v2 = live2d_v2_core::Model::new()
                .map_err(|e| anyhow::anyhow!("create V2 model: {e}"))?;
            v2.load_json(&model_json_path.to_string_lossy())
                .map_err(|e| anyhow::anyhow!("V2 load_json: {e}"))?;
            v2.resize(init_size.0 as i32, init_size.1 as i32);

            // Parse V2 hit_areas for per-part tap handling
            let hit_areas = {
                #[derive(Deserialize)]
                struct V2HitArea {
                    name: String,
                    id: String,
                }
                #[derive(Deserialize)]
                struct V2ModelJson {
                    hit_areas: Option<Vec<V2HitArea>>,
                }
                let json_text = std::fs::read_to_string(&model_json_path).unwrap_or_default();
                serde_json::from_str::<V2ModelJson>(&json_text)
                    .ok()
                    .and_then(|p| p.hit_areas)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|a| crate::model_loader::HitArea {
                        name: a.name,
                        id: a.id,
                    })
                    .collect::<Vec<_>>()
            };

            let cw = v2.canvas_width();
            let ch = v2.canvas_height();

            log::info!(
                "[pet/wayland] V2 model loaded, canvas=({:.0},{:.0}), hit_areas={}, configured size: {:?}",
                cw, ch, hit_areas.len(), state.configured_size
            );

            PetModel::V2 {
                v2_model: v2,
                v2_vao,
                last_size: (init_size.0 as i32, init_size.1 as i32),
                hit_areas,
            }
        }
    };

    // Enter event loop
    run_event_loop(
        &mut state,
        &mut event_queue,
        &qh,
        gl,
        egl_surface,
        gl_context,
        pet_model,
        cmd_rx,
        event_tx,
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Real event loop (Task 5)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_event_loop(
    state: &mut PetState,
    event_queue: &mut EventQueue<PetState>,
    qh: &QueueHandle<PetState>,
    gl: glow::Context,
    egl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
    gl_context: glutin::context::PossiblyCurrentContext,
    mut pet_model: PetModel,
    cmd_rx: &mpsc::Receiver<PetCommand>,
    event_tx: &mpsc::Sender<PetEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    let frame_duration = std::time::Duration::from_secs_f64(1.0 / 60.0);
    log::info!("[pet/wayland] event loop started (60 fps)");

    let mut prev_look_time = std::time::Instant::now();
    'frame: loop {
        let frame_start = std::time::Instant::now();

        // Snapshot current margin for this frame.  All Motion events dispatched
        // below compute offset from frame_margin_right, which stays constant
        // throughout the dispatch batch.  After this frame's commits are flushed,
        // margin_right is used as the next frame's starting point.
        state.ptr.frame_margin_right = state.ptr.margin_right;
        state.ptr.frame_margin_bottom = state.ptr.margin_bottom;

        // 1. Dispatch pending Wayland events (configure, closed, etc.)
        event_queue.dispatch_pending(state)?;

        // 2. Check for commands from main thread (non-blocking)
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                PetCommand::Exit => {
                    log::info!("[pet/wayland] received exit command");
                    let _ = event_tx.send(PetEvent::Exited);
                    break 'frame;
                }
                PetCommand::Enter { .. } => {
                    // Already in pet mode, ignore
                }
                PetCommand::SetClickThrough(enabled) => {
                    if enabled {
                        let empty_region = state.compositor.create_region(qh, ());
                        state.surface.set_input_region(Some(&empty_region));
                    } else {
                        state.surface.set_input_region(None);
                    }
                    state.surface.commit();
                    log::info!(
                        "[pet/wayland] click-through: {}",
                        if enabled { "on" } else { "off" }
                    );
                }
                PetCommand::SetParameters {
                    values,
                    part_opacities,
                } => {
                    if let PetModel::V3 { ref mut model, .. } = &mut pet_model {
                        let mut params = model.parameters();
                        let mut param_slice = params.values_mut();
                        let dst = param_slice.as_mut_slice();
                        let copy_len = dst.len().min(values.len());
                        dst[..copy_len].copy_from_slice(&values[..copy_len]);

                        if !part_opacities.is_empty() {
                            let mut parts = model.parts();
                            let opacities = parts.opacities_mut();
                            let copy_len = opacities.len().min(part_opacities.len());
                            opacities[..copy_len].copy_from_slice(&part_opacities[..copy_len]);
                        }
                    }
                }
            }
        }

        // 3. Render frame
        let size = state.configured_size.unwrap_or((400, 500));
        unsafe {
            gl.viewport(0, 0, size.0 as i32, size.1 as i32);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }

        match &mut pet_model {
            PetModel::V3 {
                model,
                renderer,
                camera,
                look,
                param_lookup,
                ..
            } => {
                // V3 local look: self-contained like V2's drag() — no main-thread dependency
                let (px, py) = (state.ptr.pointer_x as f32, state.ptr.pointer_y as f32);
                let (cw, ch) = (size.0 as f32, size.1 as f32);
                let ndc_x = 2.0 * px / cw - 1.0;
                let ndc_y = 1.0 - 2.0 * py / ch;
                look.set_target(ndc_x, ndc_y);

                let now = std::time::Instant::now();
                let dt = (now - prev_look_time).as_secs_f32().min(0.1);
                prev_look_time = now;

                {
                    let mut params = model.parameters();
                    let mut vals = params.values_mut();
                    let slice = vals.as_mut_slice();
                    for p in &look.params {
                        if let Some(&idx) = param_lookup.get(&p.id) {
                            if idx < slice.len() {
                                slice[idx] -= p.current_offset;
                            }
                        }
                    }
                }
                look.compute_raw(dt);
                {
                    let mut params = model.parameters();
                    let mut vals = params.values_mut();
                    let slice = vals.as_mut_slice();
                    for p in &look.params {
                        if let Some(&idx) = param_lookup.get(&p.id) {
                            if idx < slice.len() {
                                slice[idx] += p.current_offset;
                            }
                        }
                    }
                }

                model.update();

                // Forward tap to main thread (it has model, motion, V2/V2 dispatch)
                if let Some((cx, cy)) = state.ptr.pending_click.take() {
                    let _ = event_tx.send(PetEvent::Tap {
                        x: cx,
                        y: cy,
                        w: size.0 as f32,
                        h: size.1 as f32,
                        cam_scale_x: camera.scale_x,
                        cam_scale_y: camera.scale_y,
                        cam_translate_x: camera.translate_x,
                        cam_translate_y: camera.translate_y,
                    });
                }

                unsafe {
                    renderer.render(&gl, model, camera);
                }
            }
            PetModel::V2 {
                v2_model,
                v2_vao,
                last_size,
                hit_areas,
            } => {
                // V2 head tracking (drag manager converts screen → scene internally)
                v2_model.drag(state.ptr.pointer_x as f32, state.ptr.pointer_y as f32);

                // V2 tap: iterate hit areas and test each drawable ID
                if let Some((cx, cy)) = state.ptr.pending_click.take() {
                    if hit_areas.is_empty() {
                        // Fallback: whole-body tap via hit_part
                        let ids = v2_model.hit_part(cx as f32, cy as f32, false);
                        if !ids.is_empty() {
                            for group in ["tap_body", "tap_face", "tap_breast",
                                          "tap_belly", "tap_leg", "flick_head"] {
                                v2_model.start_random_motion(group, 3);
                            }
                        }
                    } else {
                        for area in hit_areas {
                            if v2_model.hit_test(&area.id, cx as f32, cy as f32) {
                                log::info!(
                                    "[pet/V2 tap] area={} drawable={}",
                                    area.name, area.id,
                                );
                                v2_model.start_random_motion(&format!("tap_{}", area.name), 3);
                                if area.name == "head" {
                                    v2_model.start_random_motion("flick_head", 3);
                                }
                                break;
                            }
                        }
                    }
                }

                let (vw, vh) = (size.0 as i32, size.1 as i32);
                if (vw, vh) != *last_size {
                    v2_model.resize(vw, vh);
                    *last_size = (vw, vh);
                }
                unsafe {
                    gl.bind_vertex_array(Some(*v2_vao));
                }
                v2_model.update();
                v2_model.draw();
                unsafe {
                    // Reset GL state V2 left dirty
                    gl.bind_vertex_array(None);
                    while gl.get_error() != glow::NO_ERROR {}
                    gl.use_program(None);
                    gl.active_texture(glow::TEXTURE0);
                    gl.front_face(glow::CCW);
                    gl.blend_equation_separate(glow::FUNC_ADD, glow::FUNC_ADD);
                    gl.blend_func_separate(
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                    );
                }
            }
        }

        // 4. Swap buffers
        egl_surface.swap_buffers(&gl_context)?;

        // 5. Frame rate control
        let elapsed = frame_start.elapsed();
        if elapsed < frame_duration {
            std::thread::sleep(frame_duration - elapsed);
        }
    }

    // Drop GL resources in safe order: model resources first, then context
    match pet_model {
        PetModel::V3 {
            _moc,
            model,
            renderer,
            camera,
            ..
        } => {
            drop((renderer, model, _moc, camera));
        }
        PetModel::V2 {
            v2_model, v2_vao, ..
        } => {
            drop((v2_model, v2_vao));
        }
    }
    drop((gl, egl_surface, gl_context));
    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Spawn a separate thread that creates an sctk layer-shell surface + GL context.
///
/// Returns a `JoinHandle` for the pet thread.
/// The caller sends commands via `cmd_tx` to control the thread lifecycle.
pub fn spawn_pet_surface(
    cmd_rx: mpsc::Receiver<PetCommand>,
    event_tx: mpsc::Sender<PetEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        log::info!("[pet/wayland] thread started");
        // Wait for Enter command
        match cmd_rx.recv() {
            Ok(PetCommand::Enter {
                model_dir,
                model_format,
                click_through,
            }) => {
                log::info!("[pet/wayland] enter: {:?}", model_dir);
                if let Err(e) =
                    setup_pet_surface(&model_dir, model_format, click_through, &cmd_rx, &event_tx)
                {
                    log::error!("[pet/wayland] setup error: {:?}", e);
                    let _ = event_tx.send(PetEvent::Error(format!("{:#}", e)));
                }
            }
            Ok(PetCommand::SetClickThrough(_)) => {
                // Received before Enter (toggled before pet mode started) — ignore
                log::info!("[pet/wayland] click-through ignored (no surface yet)");
            }
            Ok(PetCommand::Exit) | Err(_) => {
                log::info!("[pet/wayland] exited before enter");
                return;
            }
            Ok(_) => {
                log::warn!("[pet/wayland] ignored command before enter");
            }
        }
        log::info!("[pet/wayland] thread ended");
    })
}
