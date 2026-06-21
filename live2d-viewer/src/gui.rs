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

    // Compact vertical toolbar on the right side — matches desktop pet style
    egui::Area::new("pet_toolbar".into())
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .movable(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_black_alpha(24))
                .rounding(4.0)
                .inner_margin(egui::Margin::symmetric(2.0, 3.0))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        // Model switch
                        let btn = egui::Button::new("\u{25c0}").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).clicked() {
                            if let Some(idx) = current_idx {
                                if idx > 0 { let _ = app.switch_to(idx - 1); }
                            }
                        }

                        let model_name = current_idx
                            .and_then(|i| app.model_list.get(i))
                            .map(|e| e.name.as_str())
                            .unwrap_or("--");
                        ui.label(egui::RichText::new(model_name).size(10.0).weak());

                        let btn = egui::Button::new("\u{25b6}").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).clicked() {
                            if let Some(idx) = current_idx {
                                if idx + 1 < app.model_list.len() { let _ = app.switch_to(idx + 1); }
                            }
                        }

                        ui.add_space(4.0);

                        // Camera controls
                        let btn = egui::Button::new("\u{21ba}").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).on_hover_text("Reset view").clicked() {
                            app.camera.reset_pan();
                        }
                        let btn = egui::Button::new("+").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).on_hover_text("Zoom in").clicked() {
                            app.camera.zoom_in();
                        }
                        let btn = egui::Button::new("-").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).on_hover_text("Zoom out").clicked() {
                            app.camera.zoom_out();
                        }

                        ui.add_space(4.0);

                        // Exit (compact)
                        let btn = egui::Button::new("\u{2716}").min_size(egui::vec2(22.0, 20.0));
                        if ui.add(btn).on_hover_text("Exit pet mode").clicked() {
                            app.pet_mode = false;
                            app.pet_mode_changed = true;
                        }
                    });
                });
        });
}
