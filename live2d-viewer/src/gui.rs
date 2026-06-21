use egui::{Context, Slider, Window};
use crate::app::AppState;

pub fn draw_ui(ctx: &Context, app: &mut AppState) {
    if app.pet_mode {
        draw_pet_ui(ctx, app);
    } else {
        draw_normal_ui(ctx, app);
    }

    // Error window always shows regardless of mode
    if let Some(err) = app.error_message.clone() {
        Window::new("Error").show(ctx, |ui| {
            ui.colored_label(egui::Color32::RED, &err);
            if ui.button("Dismiss").clicked() {
                app.error_message = None;
            }
        });
    }
}

fn draw_normal_ui(ctx: &Context, app: &mut AppState) {
    let current_idx = app.current_idx;

    // Collect model info first to avoid borrow conflict with switch_to
    let model_entries: Vec<(String, bool)> = app.model_list.iter()
        .map(|e| (e.name.clone(), e.loaded))
        .collect();

    Window::new("Model List").default_width(250.0).show(ctx, |ui| {
        for (i, (name, loaded)) in model_entries.iter().enumerate() {
            let selected = current_idx == Some(i);
            let label = format!(
                "{} {}",
                if *loaded { "\u{25cf}" } else { "\u{25cb}" },
                name,
            );
            if ui.selectable_label(selected, label).clicked() {
                if let Err(e) = app.switch_to(i) {
                    app.error_message = Some(e);
                }
            }
        }

        ui.separator();
        if ui.button("Add Model...").clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                app.add_model_dir(path);
            }
        }

        ui.separator();
        if ui.button("\u{1f43e} Pet Mode").clicked() {
            app.pet_mode = true;
            app.pet_mode_changed = true;
        }
    });

    if app.current_model.is_some() {
        Window::new("Parameters").default_width(300.0).show(ctx, |ui| {
            ui.label(format!("Parameters: {}", app.parameter_names.len()));

            // Motion status
            let motion_count = app.motion_queue.entries.len();
            if motion_count > 0 {
                ui.label(format!("Motions: {}", motion_count));
                for (i, entry) in app.motion_queue.entries.iter().enumerate() {
                    let loop_str = if entry.motion.is_loop { "loop" } else { "once" };
                    let fw = entry.cached_fade_weight;
                    ui.label(format!(
                        "  [{}] {} ({:.1}s, {}, fade={:.2})",
                        i,
                        entry.motion.data.duration,
                        entry.motion.data.duration,
                        loop_str,
                        fw,
                    ));
                }
                ui.separator();
            }

            // Expression status
            if app.expression_manager.is_active {
                ui.label("Expression: active");
                ui.separator();
            }

            // Action buttons
            ui.horizontal(|ui| {
                if ui.button("Replay Idle").clicked() {
                    app.start_motion("Idle", Some(0));
                }
                if ui.button("Stop All").clicked() {
                    app.motion_queue.stop_all_motions();
                }
            });

            if let Some(tap_motions) = app.loaded_motions.get("TapBody") {
                if !tap_motions.is_empty() && ui.button("Tap Body").clicked() {
                    let idx = (app.motion_queue.user_time_seconds as usize) % tap_motions.len();
                    app.start_motion("TapBody", Some(idx));
                }
            }

            ui.separator();

            // Parameter sliders
            for i in 0..app.parameter_names.len() {
                let name = &app.parameter_names[i];
                let min = app.parameter_mins.get(i).copied().unwrap_or(-1.0);
                let max = app.parameter_maxs.get(i).copied().unwrap_or(1.0);
                let mut val = app.parameter_values[i];
                if ui.add(Slider::new(&mut val, min..=max).text(name)).changed() {
                    app.parameter_values[i] = val;
                    app.update_parameters();
                }
            }
        });
    }
}

fn draw_pet_ui(ctx: &Context, app: &mut AppState) {
    let current_idx = app.current_idx;

    // Debug: log positioning info
    let screen_rect = ctx.screen_rect();
    log::info!(
        "[pet] screen_rect=({:.0},{:.0},{:.0},{:.0}) canvas=({:.0},{:.0}) window=({:.0},{:.0}) delay={}",
        screen_rect.min.x, screen_rect.min.y, screen_rect.max.x, screen_rect.max.y,
        app.canvas_pixel_size.0, app.canvas_pixel_size.1,
        app.window_size.0, app.window_size.1,
        app.pet_mode_delay,
    );

    // Delay toolbar appearance to let window resize settle
    if app.pet_mode_delay > 0 {
        app.pet_mode_delay -= 1;
        return;
    }

    // Position toolbar at model's right edge, vertically centered.
    // Model center = window center. Model display width = canvas_w/canvas_h * window_h.
    // Right edge = (window_w + model_display_w) / 2, then convert to logical.
    let (toolbar_x, toolbar_y) = {
        let (cw, ch) = app.canvas_pixel_size;
        let (ww, wh) = app.window_size;
        let logical_w = screen_rect.width();
        let logical_h = screen_rect.height();
        if cw > 0.0 && ch > 0.0 && ww > 0.0 {
            let sf = ww / logical_w;
            let model_display_w = cw / ch * wh;
            let model_right_px = (ww + model_display_w) / 2.0;
            let x = model_right_px / sf + 4.0;
            let y = logical_h / 2.0 - 70.0; // vertically centered (toolbar ~140px tall)
            (x, y)
        } else {
            (logical_w - 36.0, 8.0)
        }
    };

    egui::Area::new("pet_toolbar".into())
        .fixed_pos(egui::pos2(toolbar_x, toolbar_y))
        .order(egui::Order::Foreground)
        .movable(false)
        .show(ctx, |ui| {
            let mut frame = egui::Frame::none();
            frame.fill = egui::Color32::from_black_alpha(20);
            frame.rounding = egui::Rounding::same(4.0);
            frame.inner_margin = egui::Margin::symmetric(2.0, 3.0);
            frame.show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    small_btn(ui, "\u{25c0}").clicked().then(|| {
                        if let Some(idx) = current_idx {
                            if idx > 0 { let _ = app.switch_to(idx - 1); }
                        }
                    });

                    ui.label(egui::RichText::new(
                        current_idx.and_then(|i| app.model_list.get(i)).map(|e| e.name.as_str()).unwrap_or("--")
                    ).size(10.0).weak());

                    small_btn(ui, "\u{25b6}").clicked().then(|| {
                        if let Some(idx) = current_idx {
                            if idx + 1 < app.model_list.len() { let _ = app.switch_to(idx + 1); }
                        }
                    });

                    ui.add_space(3.0);

                    small_btn(ui, "\u{21ba}").clicked().then(|| app.camera.reset_pan());
                    small_btn(ui, "+").clicked().then(|| app.camera.zoom_in());
                    small_btn(ui, "-").clicked().then(|| app.camera.zoom_out());

                    ui.add_space(3.0);

                    small_btn(ui, "\u{2716}").clicked().then(|| {
                        app.pet_mode = false;
                        app.pet_mode_changed = true;
                    });
                });
            });
        });
}

fn small_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(label).min_size(egui::vec2(24.0, 22.0)))
}
