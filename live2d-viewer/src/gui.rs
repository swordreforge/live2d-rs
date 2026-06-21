use egui::{Context, Slider, Window};
use crate::app::AppState;

pub fn draw_ui(ctx: &Context, app: &mut AppState) {
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
            log::info!("File dialog not yet implemented");
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
                    // Choose a random tap motion
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

    if let Some(err) = app.error_message.clone() {
        Window::new("Error").show(ctx, |ui| {
            ui.colored_label(egui::Color32::RED, &err);
            if ui.button("Dismiss").clicked() {
                app.error_message = None;
            }
        });
    }
}
