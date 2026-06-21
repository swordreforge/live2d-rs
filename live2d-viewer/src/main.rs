mod app;
mod camera;
mod gui;
mod model_loader;
mod renderer;
mod texture;

use std::sync::Arc;
use std::num::NonZeroU32;
use std::path::PathBuf;
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;
use winit::event::{Event, WindowEvent, ElementState};
use raw_window_handle::{HasRawWindowHandle, HasRawDisplayHandle};
use glutin::prelude::*;
use glutin::display::{Display, DisplayApiPreference};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext};
use glutin::surface::{SurfaceAttributesBuilder, GlSurface, WindowSurface};
use glow::HasContext;


fn main() -> anyhow::Result<()> {
    env_logger::init();

    let event_loop = EventLoop::new()?;

    let window = Arc::new(WindowBuilder::new()
        .with_title("Live2D Viewer")
        .build(&event_loop)?);

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

    #[allow(clippy::arc_with_non_send_sync)]
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|s| {
            let c_str = std::ffi::CString::new(s).expect("gl proc name");
            gl_display.get_proc_address(&c_str) as *const _
        })
    });

    let mut app = app::AppState::new();
    let mut camera = camera::Camera::new();
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

    // Try loading default models from Samples/Resources
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
    }

    event_loop.run(move |event, target| {

        match event {
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::RedrawRequested => {
                        let size = window.inner_size();
                        let clear_color = egui::Color32::from_rgb(0x1a, 0x1a, 0x2e);

                        app.update_parameters();

                        if app.current_idx != prev_idx {
                            if let Some(ref model) = app.current_model {
                                let canvas = model.canvas_info();
                                camera.fit_to_canvas(
                                    canvas.size_in_pixels.X,
                                    canvas.size_in_pixels.Y,
                                    size.width as f32,
                                    size.height as f32,
                                );
                            }
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
                                1.0,
                            );
                            gl.clear(glow::COLOR_BUFFER_BIT);
                        }

                        if let Some(ref mut model) = app.current_model {
                            unsafe {
                                renderer.render(&gl, model, &camera);
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

                        let _ = surface.swap_buffers(&gl_context);
                    }
                    _ => {
                        let egui_consumed = egui_state.on_window_event(&window, &event).consumed;

                        if !egui_consumed {
                            match event {
                                WindowEvent::CloseRequested => {
                                    target.exit();
                                }
                                WindowEvent::Resized(size) => {
                                    if let (Some(w), Some(h)) = (
                                        NonZeroU32::new(size.width),
                                        NonZeroU32::new(size.height),
                                    ) {
                                        surface.resize(&gl_context, w, h);
                                    }
                                    unsafe { gl.viewport(0, 0, size.width as i32, size.height as i32); }
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
                                    let d = match delta {
                                        winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32,
                                    };
                                    camera.zoom(d, 0.5, 0.5);
                                }
                                WindowEvent::CursorMoved { position, .. } => {
                                    let (mx, my) = (position.x, position.y);
                                    if app.mouse_down {
                                        let dx = mx - app.last_mouse_x;
                                        let dy = my - app.last_mouse_y;
                                        camera.pan(dx as f32, dy as f32);
                                    }
                                    app.last_mouse_x = mx;
                                    app.last_mouse_y = my;
                                }
                                WindowEvent::MouseInput { state, .. } => {
                                    app.mouse_down = state == ElementState::Pressed;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::AboutToWait => {
                window.request_redraw();
            }
            _ => {}
        }
    })?;
    Ok(())
}
