mod app;
mod camera;
mod data_dir;
mod db;
mod gui;
mod model_loader;
pub mod motion;
mod renderer;
mod texture;
mod tray;

use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext};
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{GlSurface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use winit::event::{ElementState, Event, WindowEvent};
use winit::platform::x11::WindowBuilderExtX11;
use winit::window::WindowBuilder;
use winit::window::WindowLevel;

fn main() -> anyhow::Result<()> {
    // Prefer X11 backend on Wayland — winit's X11 backend supports
    // window control features (name, transparency) used below.
    let on_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
    if on_wayland && std::env::var("WINIT_UNIX_BACKEND").is_err() {
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        log::info!("Wayland detected: using X11 backend for winit window control");
    }

    env_logger::init();

    // Initialize user data directory and database
    let _data_dir = data_dir::ensure_data_dir()?;
    let db = db::AppDb::open(&data_dir::db_path())?;

    // Parse CLI args: --overlay flag optional, then model path
    let mut args: Vec<String> = std::env::args().collect();
    let mut overlay_mode = false;
    args.retain(|a| {
        if a == "--overlay" {
            overlay_mode = true;
            false
        } else {
            true
        }
    });

    let event_loop =
        winit::event_loop::EventLoopBuilder::<tray::AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let (_tray, tray_rx) = tray::create_tray();

    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Live2D Viewer")
            .with_name("live2d-viewer", "live2d-viewer")
            .with_transparent(true)
            .build(&event_loop)?,
    );

    // Overlay mode: small window always-on-top at bottom-right corner
    if overlay_mode {
        window.set_decorations(false);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        let sf = window.scale_factor();
        let monitor = window.current_monitor();
        if let Some(mon) = monitor {
            let phys = mon.size();
            let log_w = phys.width as f64 / sf;
            let log_h = phys.height as f64 / sf;
            let w = 300.0f64;
            let h = 400.0f64;
            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(w, h));
            window.set_outer_position(winit::dpi::LogicalPosition::new(
                log_w - w - 10.0,
                log_h - h - 10.0,
            ));
        }
    }

    let display_handle = window.raw_display_handle();
    let window_handle = window.raw_window_handle();

    let gl_display = unsafe { Display::new(display_handle, DisplayApiPreference::Egl)? };

    let template = ConfigTemplateBuilder::new().with_alpha_size(8).build();
    let gl_config = unsafe {
        gl_display
            .find_configs(template)?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no suitable GL config"))?
    };

    let context_attrs = ContextAttributesBuilder::new().build(Some(window_handle));
    let not_current = unsafe { gl_display.create_context(&gl_config, &context_attrs)? };

    let (init_w, init_h) = {
        let size = window.inner_size();
        (
            NonZeroU32::new(size.width).unwrap_or(NonZeroU32::new(1).unwrap()),
            NonZeroU32::new(size.height).unwrap_or(NonZeroU32::new(1).unwrap()),
        )
    };
    let surf_attrs =
        SurfaceAttributesBuilder::<WindowSurface>::new().build(window_handle, init_w, init_h);
    let surface = unsafe { gl_display.create_window_surface(&gl_config, &surf_attrs)? };

    let gl_context = not_current.make_current(&surface)?;
    let _ = surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

    // Initialize V2's glad OpenGL loader (must happen after GL context is current)
    if live2d_v2_core::gl_init() == 0 {
        log::warn!("V2 glInit returned 0 — V2 rendering may not work");
    }

    #[allow(clippy::arc_with_non_send_sync)]
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|s| {
            let c_str = std::ffi::CString::new(s).expect("gl proc name");
            gl_display.get_proc_address(&c_str) as *const _
        })
    });

    // Create a VAO for V2 rendering (V2 uses core-profile-incompatible no-VAO GL 2.1 pattern)
    let v2_vao = unsafe { gl.create_vertex_array().expect("create V2 VAO") };

    let mut app = app::AppState::new(Some(db));
    let mut renderer = unsafe {
        renderer::Live2dRenderer::new(&gl).map_err(|e| anyhow::anyhow!("renderer: {e}"))?
    };

    // Floating button overlay (raw GL, bypasses egui_glow coordinate bug)
    let mut float_overlay = renderer::FloatOverlay::new();

    // egui setup
    let egui_ctx = egui::Context::default();

    // Load CJK font to fix □□□ (tofu) for Chinese/Japanese text
    if let Ok(cjk_data) =
        std::fs::read("/usr/share/fonts/adobe-source-han-sans/SourceHanSansCN-Medium.otf")
    {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("CJK".into(), egui::FontData::from_owned(cjk_data));
        // Append CJK as fallback (after Latin+Emoji) so CJK chars get rendered
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push("CJK".into());
            }
        }
        egui_ctx.set_fonts(fonts);
    }
    let mut painter = egui_glow::Painter::new(gl.clone(), "", None)
        .map_err(|e| anyhow::anyhow!("painter: {:?}", e))?;
    let mut egui_state = egui_winit::State::new(
        egui_ctx.clone(),
        egui::ViewportId::ROOT,
        &*window,
        None,
        Some(painter.max_texture_side()),
    );

    let mut prev_idx: Option<usize> = None;

    let model_path_arg = args.get(1).cloned();
    let model_loaded = if let Some(arg) = model_path_arg {
        let model_dir = PathBuf::from(&arg);
        if model_dir.exists() {
            let name = model_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("model");
            let name_owned = name.to_string();
            let cli_format = app::detect_model_format(&model_dir);
            app.model_list.push(app::ModelEntry {
                name: name.into(),
                dir: model_dir,
                loaded: false,
                format: cli_format,
            });
            // Record CLI model in DB
            if let Some(ref db) = app.db {
                let model_version = match cli_format {
                    Some(app::ModelFormat::V3) => "V3",
                    Some(app::ModelFormat::V2) => "V2",
                    None => "Unknown",
                };
                let _ = db.add_or_update_model(&arg, &name_owned, model_version, None);
            }
            true
        } else {
            eprintln!("model directory not found: {arg}");
            false
        }
    } else {
        false
    };

    if !model_loaded {
        // 运行时模型扫描：仅当 LIVE2D_SDK_ROOT 设了才扫描 Samples
        let samples_resources = std::env::var("LIVE2D_SDK_ROOT")
            .map(|root| PathBuf::from(root).join("Samples").join("Resources"))
            .unwrap_or_else(|_| PathBuf::new());

        if samples_resources.exists() {
            if let Ok(entries) = std::fs::read_dir(&samples_resources) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && app::detect_model_format(&path).is_some() {
                        app.add_model_dir(path);
                    }
                }
            }
        }
    }

    // Restore model history from DB (merge into model_list, prefer existing entries)
    if let Ok(records) = app
        .db
        .as_ref()
        .map(|db| db.model_history())
        .unwrap_or(Ok(Vec::new()))
    {
        for rec in records {
            let p = std::path::PathBuf::from(&rec.file_path);
            if p.exists() && !app.model_list.iter().any(|e| e.dir == p) {
                let fmt = app::detect_model_format(&p);
                app.model_list.push(app::ModelEntry {
                    name: rec.name.clone(),
                    dir: p,
                    loaded: false,
                    format: fmt,
                });
            }
        }
    }

    if !app.model_list.is_empty() {
        let _ = app.switch_to(0);
        for path in &app.texture_paths {
            match std::fs::read(path) {
                Ok(img_data) => match unsafe { texture::load_texture(&gl, &img_data) } {
                    Ok(tex) => renderer.textures.push(tex),
                    Err(e) => eprintln!("texture load {:?}: {e}", path),
                },
                Err(e) => eprintln!("texture read {:?}: {e}", path),
            }
        }
    }

    // Frame timing
    let mut last_frame_time = Instant::now();

    event_loop.run(move |event, target| {
        match event {
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::RedrawRequested => {
                        // --- Complete any pending async model switch ---
                        app.complete_pending_switch();

                        // --- Frame timing ---
                        let now = Instant::now();
                        let delta = now.duration_since(last_frame_time).as_secs_f32().min(0.1); // cap at 100ms
                        last_frame_time = now;

                        // --- Helper: request window sized to model display, clamped to monitor ---
                        fn request_model_window(window: &winit::window::Window, cw: f32, ch: f32) {
                            let sf = window.scale_factor();
                            let max_lh = window
                                .current_monitor()
                                .map(|m| m.size().height as f64 / sf - 40.0)
                                .unwrap_or(800.0)
                                .max(200.0);
                            let target_h = max_lh * 0.9;
                            let model_display_w = target_h as f32 * cw / ch;
                            let target_w = (model_display_w * 1.1 + 50.0) as f64; // 10% padding + toolbar
                            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(
                                target_w, target_h,
                            ));
                        }

                        // --- Apply pending pet mode window changes ---
                        if app.pet_mode_changed {
                            if app.pet_mode {
                                window.set_decorations(false);
                                window.set_window_level(WindowLevel::AlwaysOnTop);
                                if let Some(ref model) = app.current_model {
                                    let canvas = model.canvas_info();
                                    request_model_window(
                                        &window,
                                        canvas.size_in_pixels.X,
                                        canvas.size_in_pixels.Y,
                                    );
                                }
                                log::info!(
                                    "[pet] enter: canvas=({:.0},{:.0})",
                                    app.canvas_pixel_size.0,
                                    app.canvas_pixel_size.1
                                );
                                app.camera_needs_fit = true;
                                app.pet_mode_delay = 2;
                            } else {
                                window.set_decorations(true);
                                window.set_window_level(WindowLevel::Normal);
                                log::info!("[pet] exit");
                            }
                            app.pet_mode_changed = false;
                        }

                        // --- Resize window when model switches in pet mode ---
                        if app.pet_resize_pending {
                            app.pet_resize_pending = false;
                            app.pet_mode_delay = 2;
                            if app.pet_mode {
                                if let Some(ref model) = app.current_model {
                                    let canvas = model.canvas_info();
                                    request_model_window(
                                        &window,
                                        canvas.size_in_pixels.X,
                                        canvas.size_in_pixels.Y,
                                    );
                                }
                            }
                        }

                        // --- Camera recalculation (model switch OR pending pet mode resize) ---
                        // This runs BEFORE pet_mode_changed so that camera_needs_fit set by
                        // pet mode activation is consumed on the NEXT frame (after resize settles).
                        if app.current_idx != prev_idx || app.camera_needs_fit {
                            let size = window.inner_size();
                            if let Some(ref model) = app.current_model {
                                let canvas = model.canvas_info();
                                app.camera.fit_to_canvas(
                                    canvas.size_in_pixels.X,
                                    canvas.size_in_pixels.Y,
                                    canvas.pixels_per_unit,
                                    size.width as f32,
                                    size.height as f32,
                                );
                            }
                            app.camera_needs_fit = false;
                        }

                        // --- Advance motion system (V3 only, skip when floating) ---
                        if !app.is_v2 && !app.minimized_to_float {
                            app.advance_motion(delta);
                            app.update_parameters();
                            app.update_pose(delta);
                        }

                        let size = window.inner_size();
                        app.window_size = (size.width as f32, size.height as f32);
                        let clear_color = if app.minimized_to_float {
                            egui::Color32::from_rgb(0x33, 0x99, 0xff)
                        } else if app.pet_mode {
                            egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                        } else {
                            egui::Color32::from_rgb(0x1a, 0x1a, 0x2e)
                        };

                        // Texture reload on model switch only
                        if app.current_idx != prev_idx {
                            unsafe {
                                for tex in renderer.textures.drain(..) {
                                    gl.delete_texture(tex);
                                }
                            }
                            for path in &app.texture_paths {
                                match std::fs::read(path) {
                                    Ok(img_data) => {
                                        match unsafe { texture::load_texture(&gl, &img_data) } {
                                            Ok(tex) => renderer.textures.push(tex),
                                            Err(e) => eprintln!("texture load {:?}: {e}", path),
                                        }
                                    }
                                    Err(e) => eprintln!("texture read {:?}: {e}", path),
                                }
                            }
                            prev_idx = app.current_idx;
                        }

                        unsafe {
                            gl.clear_color(
                                clear_color.r() as f32 / 255.0,
                                clear_color.g() as f32 / 255.0,
                                clear_color.b() as f32 / 255.0,
                                clear_color.a() as f32 / 255.0,
                            );
                            gl.clear(glow::COLOR_BUFFER_BIT);
                        }

                        if app.is_v2 {
                            if let Some(ref mut v2) = app.v2_model {
                                if !app.minimized_to_float {
                                    unsafe {
                                        gl.viewport(0, 0, size.width as i32, size.height as i32);
                                        // Bind VAO so V2's glVertexAttribPointer works (V2 uses GL 2.1 style without VAO)
                                        gl.bind_vertex_array(Some(v2_vao));
                                    }
                                    let (vw, vh) = (size.width as i32, size.height as i32);
                                    if (vw, vh) != app.last_v2_size {
                                        v2.resize(vw, vh);
                                        app.last_v2_size = (vw, vh);
                                    }
                                    v2.update();
                                    v2.draw();
                                    unsafe {
                                        // Reset GL state V2 left dirty, drain stale errors
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
                        } else if let Some(ref mut model) = app.current_model {
                            if !app.minimized_to_float {
                                unsafe {
                                    gl.viewport(0, 0, size.width as i32, size.height as i32);
                                    gl.disable(glow::DEPTH_TEST);
                                    gl.disable(glow::CULL_FACE);
                                    renderer.render(&gl, model, &app.camera);
                                }
                            }
                        }

                        // --- Egui frame (always runs for input, rendering skipped when floating) ---
                        unsafe {
                            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                            gl.viewport(0, 0, size.width as i32, size.height as i32);
                        }
                        let raw_input = egui_state.take_egui_input(&window);
                        egui_ctx.begin_frame(raw_input);
                        gui::draw_ui(&egui_ctx, &mut app);
                        let output = egui_ctx.end_frame();

                        // Always process textures
                        for (id, delta) in &output.textures_delta.set {
                            painter.set_texture(*id, delta);
                        }
                        for id in &output.textures_delta.free {
                            painter.free_texture(*id);
                        }

                        // Render shapes: egui_glow when normal, raw GL triangle when floating
                        if app.minimized_to_float {
                            unsafe {
                                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                                gl.viewport(0, 0, size.width as i32, size.height as i32);
                                float_overlay.draw_play_button(
                                    &gl,
                                    size.width as f32,
                                    size.height as f32,
                                );
                            }
                        } else {
                            let clipped_primitives =
                                egui_ctx.tessellate(output.shapes, output.pixels_per_point);
                            painter.paint_primitives(
                                [size.width, size.height],
                                output.pixels_per_point,
                                &clipped_primitives,
                            );
                        }

                        // Minimize (X11 → hide; Wayland → small float window)
                        if app.request_minimize {
                            app.request_minimize = false;
                            let on_x11 = matches!(
                                window.raw_window_handle(),
                                raw_window_handle::RawWindowHandle::Xlib(_)
                                    | raw_window_handle::RawWindowHandle::Xcb(_)
                            );
                            if on_x11 {
                                window.set_visible(false);
                            } else {
                                app.minimized_to_float = true;
                                let sf = window.scale_factor();
                                app.saved_window_pet_size =
                                    (app.window_size.0 as f64 / sf, app.window_size.1 as f64 / sf);
                                app.camera_needs_fit = false;
                                window.set_max_inner_size(Some(winit::dpi::LogicalSize::new(
                                    50.0, 50.0,
                                )));
                                let _ = window
                                    .request_inner_size(winit::dpi::LogicalSize::new(50.0, 50.0));
                                // Force EGL surface resize immediately (Wayland workaround)
                                let phys_w = (50.0_f64 * sf) as u32;
                                let phys_h = (50.0_f64 * sf) as u32;
                                if let (Some(rw), Some(rh)) =
                                    (NonZeroU32::new(phys_w), NonZeroU32::new(phys_h))
                                {
                                    surface.resize(&gl_context, rw, rh);
                                }
                            }
                        }
                        if app.request_restore {
                            app.request_restore = false;
                            if app.minimized_to_float {
                                app.minimized_to_float = false;
                                let (w, h) = app.saved_window_pet_size;
                                let rw = w.clamp(200.0, 4000.0);
                                let rh = h.clamp(200.0, 4000.0);
                                window.set_max_inner_size(None::<winit::dpi::LogicalSize<f64>>);
                                let _ =
                                    window.request_inner_size(winit::dpi::LogicalSize::new(rw, rh));
                                let sf = window.scale_factor();
                                let phys_w = (rw * sf) as u32;
                                let phys_h = (rh * sf) as u32;
                                if let (Some(pw), Some(ph)) =
                                    (NonZeroU32::new(phys_w), NonZeroU32::new(phys_h))
                                {
                                    surface.resize(&gl_context, pw, ph);
                                }
                                app.camera_needs_fit = true;
                            } else {
                                window.set_visible(true);
                            }
                        }

                        let _ = surface.swap_buffers(&gl_context);
                    }
                    _ => {
                        // Always track mouse for look (before egui may consume the event)
                        if let WindowEvent::CursorMoved { position, .. } = &event {
                            let (mx, my) = (position.x, position.y);
                            app.last_mouse_x = mx;
                            app.last_mouse_y = my;
                            let size = window.inner_size();
                            app.update_mouse_for_look(
                                mx,
                                my,
                                size.width as f32,
                                size.height as f32,
                            );
                            // V2 head tracking: feed mouse to drag manager (screen → scene internally)
                            if app.is_v2 {
                                if let Some(ref mut v2) = app.v2_model {
                                    v2.drag(mx as f32, my as f32);
                                }
                            }
                        }

                        let egui_consumed = egui_state.on_window_event(&window, &event).consumed;

                        if !egui_consumed {
                            match event {
                                WindowEvent::CloseRequested => {
                                    if app.pet_mode {
                                        if app.minimized_to_float {
                                            app.request_restore = true;
                                        } else {
                                            window.set_visible(false);
                                        }
                                    } else {
                                        target.exit();
                                    }
                                }
                                WindowEvent::Resized(size) => {
                                    if app.minimized_to_float {
                                        let float_logical = 50.0;
                                        let max_phys =
                                            (float_logical * window.scale_factor()).ceil() as u32;
                                        if size.width > max_phys || size.height > max_phys {
                                            let _ = window.request_inner_size(
                                                winit::dpi::LogicalSize::new(
                                                    float_logical,
                                                    float_logical,
                                                ),
                                            );
                                            if let (Some(rw), Some(rh)) = (
                                                NonZeroU32::new(max_phys.max(1)),
                                                NonZeroU32::new(max_phys.max(1)),
                                            ) {
                                                surface.resize(&gl_context, rw, rh);
                                            }
                                            return;
                                        }
                                    }
                                    if let (Some(w), Some(h)) =
                                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                                    {
                                        surface.resize(&gl_context, w, h);
                                    }
                                    unsafe {
                                        gl.viewport(0, 0, size.width as i32, size.height as i32);
                                    }
                                    // V2: update matrix manager projection on resize
                                    if app.is_v2 {
                                        if let Some(ref mut v2) = app.v2_model {
                                            let (vw, vh) = (size.width as i32, size.height as i32);
                                            v2.resize(vw, vh);
                                            app.last_v2_size = (vw, vh);
                                        }
                                    }
                                    // Recalculate camera when window size changes (skip when floating)
                                    if !app.minimized_to_float {
                                        app.camera_needs_fit = true;
                                    }
                                }
                                WindowEvent::KeyboardInput { event: ref ke, .. } => {
                                    if ke.state == ElementState::Pressed {
                                        use winit::keyboard::KeyCode;
                                        if let winit::keyboard::PhysicalKey::Code(code) =
                                            ke.physical_key
                                        {
                                            if code == KeyCode::ArrowLeft {
                                                let idx = app.current_idx.unwrap_or(0);
                                                if idx > 0 {
                                                    let _ = app.begin_switch(idx - 1);
                                                }
                                            } else if code == KeyCode::ArrowRight {
                                                let idx = app.current_idx.unwrap_or(0);
                                                if idx + 1 < app.model_list.len() {
                                                    let _ = app.begin_switch(idx + 1);
                                                }
                                            }
                                        }
                                    }
                                }
                                WindowEvent::MouseWheel { delta, .. } => {
                                    if !app.pet_mode {
                                        let d = match delta {
                                            winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                                            winit::event::MouseScrollDelta::PixelDelta(p) => {
                                                p.y as f32
                                            }
                                        };
                                        if app.is_v2 {
                                            // V2 zoom via MatrixManager.setScale (tracked in app.v2_scale)
                                            app.v2_scale =
                                                (app.v2_scale + d * 0.15).clamp(0.1, 10.0);
                                            if let Some(ref mut v2) = app.v2_model {
                                                v2.set_scale(app.v2_scale);
                                            }
                                            app.save_zoom();
                                        } else {
                                            app.camera.zoom(d, 0.5, 0.5);
                                            app.save_zoom();
                                        }
                                    }
                                }
                                WindowEvent::CursorMoved { position, .. } => {
                                    // Camera pan (mouse_down in normal mode) — V2 uses drag() above instead
                                    if app.mouse_down && !app.pet_mode && !app.is_v2 {
                                        let dx = position.x - app.last_mouse_x;
                                        let dy = position.y - app.last_mouse_y;
                                        app.camera.pan(dx as f32, dy as f32);
                                    }
                                }
                                WindowEvent::MouseInput { state, .. } => {
                                    let was_down = app.mouse_down;
                                    app.mouse_down = state == ElementState::Pressed;
                                    if state == ElementState::Pressed && !was_down {
                                        let size = window.inner_size();
                                        let mx = app.last_mouse_x;
                                        let my = app.last_mouse_y;
                                        let cam_scale_x = app.camera.scale_x;
                                        let cam_scale_y = app.camera.scale_y;
                                        let cam_trans_x = app.camera.translate_x;
                                        let cam_trans_y = app.camera.translate_y;
                                        app.handle_tap_with_cam(
                                            mx,
                                            my,
                                            size.width as f32,
                                            size.height as f32,
                                            cam_scale_x,
                                            cam_scale_y,
                                            cam_trans_x,
                                            cam_trans_y,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::UserEvent(event) => match event {
                tray::AppEvent::ShowWindow => {
                    app.request_restore = true;
                }
                tray::AppEvent::Quit => {
                    target.exit();
                }
            },
            Event::LoopExiting => {
                // Save last active model path
                if let Some(idx) = app.current_idx {
                    if let Some(entry) = app.model_list.get(idx) {
                        if let Some(ref db) = app.db {
                            let _ = db.set_setting(
                                "last_active_model_path",
                                &entry.dir.to_string_lossy(),
                            );
                        }
                    }
                }
                painter.destroy();
            }
            Event::AboutToWait => {
                // Poll tray menu events and forward to main event loop
                for id in tray_rx.poll() {
                    let event = match id.as_str() {
                        "show" => tray::AppEvent::ShowWindow,
                        "quit" => tray::AppEvent::Quit,
                        _ => continue,
                    };
                    let _ = proxy.send_event(event);
                }
                window.request_redraw();
            }
            _ => {}
        }
    })?;
    Ok(())
}
