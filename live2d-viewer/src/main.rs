mod app;
mod camera;
mod gui;
mod model_loader;
pub mod motion;
mod renderer;
mod texture;
mod tray;

use std::sync::Arc;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::Instant;
use winit::platform::x11::WindowBuilderExtX11;
use winit::window::WindowBuilder;
use winit::event::{Event, WindowEvent, ElementState};
use winit::window::WindowLevel;
use raw_window_handle::{HasRawWindowHandle, HasRawDisplayHandle};
use glutin::prelude::*;
use glutin::display::{Display, DisplayApiPreference};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext};
use glutin::surface::{SurfaceAttributesBuilder, GlSurface, WindowSurface, SwapInterval};
use glow::HasContext;


fn main() -> anyhow::Result<()> {
    // Auto-switch to X11 on Wayland: winit + GTK both need X11 backend
    let on_wayland = std::env::var("WAYLAND_DISPLAY").is_ok();
    if on_wayland && std::env::var("WINIT_UNIX_BACKEND").is_err() {
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        std::env::set_var("GDK_BACKEND", "x11");
        log::info!("Wayland detected: using X11 backends for winit+GTK (tray + window control)");
    }

    env_logger::init();

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

    // Initialize GTK for tray icon (required by tray-icon GTK backend)
    let gtk_ok = gtk::init().is_ok();
    if !gtk_ok {
        log::warn!("GTK init failed — tray icon disabled");
    }


    let event_loop = winit::event_loop::EventLoopBuilder::<tray::AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let (_tray, tray_rx) = if gtk_ok {
        tray::create_tray()
    } else {
        let icon = tray_icon::Icon::from_rgba(vec![0; 4], 1, 1).unwrap();
        let dummy_menu = tray_icon::menu::Menu::new();
        let dummy_tray = tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(dummy_menu))
            .with_icon(icon)
            .build()
            .unwrap();
        (dummy_tray, tray::dummy_receiver())
    };

    let window = Arc::new(WindowBuilder::new()
        .with_title("Live2D Viewer")
        .with_name("live2d-viewer", "live2d-viewer")
        .with_transparent(true)
        .build(&event_loop)?);

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
            window.set_outer_position(winit::dpi::LogicalPosition::new(log_w - w - 10.0, log_h - h - 10.0));
        }
    }

    let display_handle = window.raw_display_handle();
    let window_handle = window.raw_window_handle();

    let gl_display = unsafe {
        Display::new(display_handle, DisplayApiPreference::Egl)?
    };

    let template = ConfigTemplateBuilder::new()
        .with_alpha_size(8)
        .build();
    let gl_config = unsafe {
        gl_display.find_configs(template)?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no suitable GL config"))?
    };

    let context_attrs = ContextAttributesBuilder::new()
        .build(Some(window_handle));
    let not_current = unsafe {
        gl_display.create_context(&gl_config, &context_attrs)?
    };

    let (init_w, init_h) = {
        let size = window.inner_size();
        (
            NonZeroU32::new(size.width).unwrap_or(NonZeroU32::new(1).unwrap()),
            NonZeroU32::new(size.height).unwrap_or(NonZeroU32::new(1).unwrap()),
        )
    };
    let surf_attrs = SurfaceAttributesBuilder::<WindowSurface>::new()
        .build(window_handle, init_w, init_h);
    let surface = unsafe {
        gl_display.create_window_surface(&gl_config, &surf_attrs)?
    };

    let gl_context = not_current.make_current(&surface)?;
    let _ = surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

    #[allow(clippy::arc_with_non_send_sync)]
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|s| {
            let c_str = std::ffi::CString::new(s).expect("gl proc name");
            gl_display.get_proc_address(&c_str) as *const _
        })
    });

    let mut app = app::AppState::new();
    let mut renderer = unsafe {
        renderer::Live2dRenderer::new(&gl)
            .map_err(|e| anyhow::anyhow!("renderer: {e}"))?
    };

    // egui setup
    let egui_ctx = egui::Context::default();
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
            let name = model_dir.file_name().and_then(|n| n.to_str()).unwrap_or("model");
            app.model_list.push(app::ModelEntry { name: name.into(), dir: model_dir, loaded: false });
            true
        } else {
            eprintln!("model directory not found: {arg}");
            false
        }
    } else {
        false
    };

    if !model_loaded {
        let samples_resources = PathBuf::from(
            std::env::var("LIVE2D_SDK_ROOT").unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{home}/Downloads/CubismSdkForNative-5-r.5")
            })
        ).join("Samples").join("Resources");

        if samples_resources.exists() {
            if let Ok(entries) = std::fs::read_dir(&samples_resources) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let has_model3 = std::fs::read_dir(&path)
                            .map(|rd| rd.flat_map(|e| e.ok()).any(|e| {
                                e.path().extension().map(|ext| ext == "json").unwrap_or(false)
                            }))
                            .unwrap_or(false);
                        if has_model3 {
                            app.add_model_dir(path);
                        }
                    }
                }
            }
        }
    }

    if !app.model_list.is_empty() {
        let _ = app.switch_to(0);
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
    }

    // Frame timing
    let mut last_frame_time = Instant::now();

    event_loop.run(move |event, target| {

        match event {
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::RedrawRequested => {
                        // --- Frame timing ---
                        let now = Instant::now();
                        let delta = now.duration_since(last_frame_time).as_secs_f32().min(0.1); // cap at 100ms
                        last_frame_time = now;

                        // --- Helper: request window sized to model display, clamped to monitor ---
                        fn request_model_window(window: &winit::window::Window, cw: f32, ch: f32) {
                            let sf = window.scale_factor();
                            let max_lh = window.current_monitor()
                                .map(|m| m.size().height as f64 / sf - 40.0)
                                .unwrap_or(800.0)
                                .max(200.0);
                            let target_h = max_lh * 0.9;
                            let model_display_w = target_h as f32 * cw / ch;
                            let target_w = (model_display_w * 1.1 + 50.0) as f64; // 10% padding + toolbar
                            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(target_w, target_h));
                        }

                        // --- Apply pending pet mode window changes ---
                        if app.pet_mode_changed {
                            if app.pet_mode {
                                window.set_decorations(false);
                                window.set_window_level(WindowLevel::AlwaysOnTop);
                                if let Some(ref model) = app.current_model {
                                    let canvas = model.canvas_info();
                                    request_model_window(&window, canvas.size_in_pixels.X, canvas.size_in_pixels.Y);
                                }
                                log::info!("[pet] enter: canvas=({:.0},{:.0})",
                                    app.canvas_pixel_size.0, app.canvas_pixel_size.1);
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
                                    request_model_window(&window, canvas.size_in_pixels.X, canvas.size_in_pixels.Y);
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

                        // --- Advance motion system (skip when floating) ---
                        if !app.minimized_to_float {
                            app.advance_motion(delta);
                            app.update_parameters();
                            app.update_pose(delta);
                        }

                        let size = window.inner_size();
                        app.window_size = (size.width as f32, size.height as f32);
                        let clear_color = if app.pet_mode {
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

                        if let Some(ref mut model) = app.current_model {
                            unsafe {
                                gl.viewport(0, 0, size.width as i32, size.height as i32);
                                gl.disable(glow::DEPTH_TEST);
                                gl.disable(glow::CULL_FACE);
                                renderer.render(&gl, model, &app.camera);
                            }
                        }

                        // --- egui overlay ---
                        {
                            let raw_input = egui_state.take_egui_input(&window);
                            egui_ctx.begin_frame(raw_input);
                            gui::draw_ui(&egui_ctx, &mut app);
                            let output = egui_ctx.end_frame();

                            for (id, delta) in &output.textures_delta.set {
                                painter.set_texture(*id, delta);
                            }

                            let clipped_primitives =
                                egui_ctx.tessellate(output.shapes, output.pixels_per_point);
                            painter.paint_primitives(
                                [size.width, size.height],
                                output.pixels_per_point,
                                &clipped_primitives,
                            );

                            for id in &output.textures_delta.free {
                                painter.free_texture(*id);
                            }
                        }

                        // Minimize (tray on X11, floating circle on native Wayland)
                        if app.request_minimize {
                            app.request_minimize = false;
                            // Detect actual backend from window handle (not env var — winit may fall back)
                            let on_x11 = match window.raw_window_handle() {
                                raw_window_handle::RawWindowHandle::Xlib(_)
                                | raw_window_handle::RawWindowHandle::Xcb(_) => true,
                                _ => false,
                            };
                            if on_x11 {
                                let _ = window.set_visible(false);
                            } else {
                                app.minimized_to_float = true;
                                let sf = window.scale_factor();
                                app.saved_window_pet_size = (
                                    app.window_size.0 as f64 / sf,
                                    app.window_size.1 as f64 / sf,
                                );
                                let _ = window.request_inner_size(winit::dpi::LogicalSize::new(150.0, 150.0));
                            }
                        }
                        if app.request_restore {
                            app.request_restore = false;
                            if app.minimized_to_float {
                                app.minimized_to_float = false;
                                let (w, h) = app.saved_window_pet_size;
                                let rw = (w.max(200.0)).min(4000.0);
                                let rh = (h.max(200.0)).min(4000.0);
                                let _ = window.request_inner_size(winit::dpi::LogicalSize::new(rw, rh));
                                app.camera_needs_fit = true;
                            } else {
                                let _ = window.set_visible(true);
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
                            app.update_mouse_for_look(mx, my, size.width as f32, size.height as f32);
                        }

                        let egui_consumed = egui_state.on_window_event(&window, &event).consumed;

                        if !egui_consumed {
                            match event {
                                WindowEvent::CloseRequested => {
                                    if app.pet_mode {
                                        if app.minimized_to_float {
                                            app.request_restore = true;
                                        } else {
                                            let _ = window.set_visible(false);
                                        }
                                    } else {
                                        target.exit();
                                    }
                                }
                                WindowEvent::Resized(size) => {
                                    if let (Some(w), Some(h)) = (
                                        NonZeroU32::new(size.width),
                                        NonZeroU32::new(size.height),
                                    ) {
                                        surface.resize(&gl_context, w, h);
                                    }
                                    unsafe { gl.viewport(0, 0, size.width as i32, size.height as i32); }
                                    // Recalculate camera when window size actually changes
                                    app.camera_needs_fit = true;
                                }
                                WindowEvent::KeyboardInput { event: ref ke, .. } => {
                                    if ke.state == ElementState::Pressed {
                                        use winit::keyboard::KeyCode;
                                        if let winit::keyboard::PhysicalKey::Code(code) = ke.physical_key {
                                            if code == KeyCode::ArrowLeft {
                                                let idx = app.current_idx.unwrap_or(0);
                                                if idx > 0 { let _ = app.switch_to(idx - 1); }
                                            } else if code == KeyCode::ArrowRight {
                                                let idx = app.current_idx.unwrap_or(0);
                                                if idx + 1 < app.model_list.len() { let _ = app.switch_to(idx + 1); }
                                            }
                                        }
                                    }
                                }
                                WindowEvent::MouseWheel { delta, .. } => {
                                    if !app.pet_mode {
                                        let d = match delta {
                                            winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                                            winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                                        };
                                        app.camera.zoom(d, 0.5, 0.5);
                                    }
                                }
                                WindowEvent::CursorMoved { position, .. } => {
                                    // Camera pan (mouse_down in normal mode)
                                    if app.mouse_down && !app.pet_mode {
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
                                            mx, my,
                                            size.width as f32, size.height as f32,
                                            cam_scale_x, cam_scale_y, cam_trans_x, cam_trans_y,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::UserEvent(event) => {
                match event {
                    tray::AppEvent::ShowWindow => {
                        app.request_restore = true;
                    }
                    tray::AppEvent::Quit => {
                        target.exit();
                    }
                }
            }
            Event::LoopExiting => {
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
