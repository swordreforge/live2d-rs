use egui::{Context, Slider, Window};
use crate::app::AppState;

pub fn draw_ui(ctx: &Context, app: &mut AppState) {
    if app.minimized_to_float {
        draw_floating_ui(ctx, app);
        return;
    }

    if app.pet_mode {
        draw_pet_ui(ctx, app);
    } else {
        draw_normal_ui(ctx, app);
    }

    // Error window always shows regardless of mode
    if let Some(err) = app.error_message.clone() {
        Window::new("错误").show(ctx, |ui| {
            ui.colored_label(egui::Color32::RED, &err);
            if ui.button("关闭").clicked() {
                app.error_message = None;
            }
        });
    }
}

fn draw_floating_ui(ctx: &Context, app: &mut AppState) {
    let screen = ctx.screen_rect();

    let btn_size = screen.size().x.min(screen.size().y) - 4.0;
    let btn_rect = egui::Rect::from_center_size(
        screen.center(),
        egui::vec2(btn_size, btn_size),
    );
    let icon_size = btn_size * 0.45;

    let painter = ctx.debug_painter();
    // Draw a RED rect to contrast with blue GL clear
    painter.rect_filled(btn_rect, 4.0, egui::Color32::RED);
    painter.text(
        btn_rect.center(),
        egui::Align2::CENTER_CENTER,
        "\u{25b6}",
        egui::FontId::proportional(icon_size),
        egui::Color32::WHITE,
    );

    if ctx.input(|i| i.pointer.any_click()) {
        let pos = match ctx.input(|i| i.pointer.interact_pos()) {
            Some(p) => p,
            None => return,
        };
        if btn_rect.contains(pos) {
            app.request_restore = true;
        }
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
                if let Err(e) = app.begin_switch(i) {
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

        // Show loading indicator during async model switch
        if matches!(app.pending_load, crate::app::PendingLoad::V3Loading(..)) {
            ui.label(egui::RichText::new("加载中...").color(egui::Color32::YELLOW));
        }

        ui.separator();
        if ui.button("\u{1f43e} Pet Mode").clicked() {
            app.pet_mode = true;
            app.pet_mode_changed = true;
        }

        // Zoom controls — always visible regardless of model type
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("-").clicked() {
                if app.is_v2 {
                    app.v2_scale = (app.v2_scale * 0.85).max(0.1);
                    if let Some(ref mut v2) = app.v2_model {
                        v2.set_scale(app.v2_scale);
                    }
                } else {
                    app.camera.zoom_out();
                }
            }
            if ui.button("Reset").clicked() {
                if app.is_v2 {
                    app.v2_scale = 1.0;
                    if let Some(ref mut v2) = app.v2_model {
                        v2.set_scale(1.0);
                        v2.set_offset(0.0, 0.0);
                    }
                } else {
                    app.camera.reset_pan();
                }
            }
            if ui.button("+").clicked() {
                if app.is_v2 {
                    app.v2_scale = (app.v2_scale * 1.15).min(10.0);
                    if let Some(ref mut v2) = app.v2_model {
                        v2.set_scale(app.v2_scale);
                    }
                } else {
                    app.camera.zoom_in();
                }
            }
        });
    });

    if app.current_model.is_some() {
        Window::new("Parameters").default_width(300.0).show(ctx, |ui| {
            ui.label(format!("Parameters: {}", app.parameter_names.len()));

            // Motion status
            let motion_count = app.motion_queue.entries.len();
            if motion_count > 0 {
                ui.label(format!("Motions: {}", motion_count));
                for (i, entry) in app.motion_queue.entries.iter().enumerate() {
                    let loop_str = if entry.motion.is_loop { "循环" } else { "一次" };
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
                ui.label("表情: 启用");
                ui.separator();
            }

            // Action buttons
            ui.horizontal(|ui| {
                if ui.button("重放待机").clicked() {
                    app.start_motion("Idle", Some(0));
                }
                if ui.button("全部停止").clicked() {
                    app.motion_queue.stop_all_motions();
                }
            });

            if let Some(tap_motions) = app.loaded_motions.get("TapBody") {
                if !tap_motions.is_empty() && ui.button("点击身体").clicked() {
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

    let screen_rect = ctx.screen_rect();

    // Delay toolbar appearance to let window resize settle
    if app.pet_mode_delay > 0 {
        app.pet_mode_delay -= 1;
        return;
    }

    // Position toolbar at model's right edge, vertically centered.
    // Clamp to window bounds so wide-aspect models don't push the panel off-screen.
    let (toolbar_x, toolbar_y) = {
        let (cw, ch) = app.canvas_pixel_size;
        let (ww, wh) = app.window_size;
        let logical_w = screen_rect.width();
        let logical_h = screen_rect.height();
        if cw > 0.0 && ch > 0.0 && ww > 0.0 {
            let sf = ww / logical_w;
            let model_display_w = cw / ch * wh;
            let model_right_px = (ww + model_display_w) / 2.0;
            let toolbar_w = 44.0;
            let x = (model_right_px / sf).clamp(2.0, logical_w - toolbar_w);
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
                            if idx > 0 { let _ = app.begin_switch(idx - 1); }
                        }
                    });

                    ui.label(egui::RichText::new(
                        if matches!(app.pending_load, crate::app::PendingLoad::V3Loading(..)) {
                            "加载中..."
                        } else {
                            current_idx.and_then(|i| app.model_list.get(i)).map(|e| e.name.as_str()).unwrap_or("--")
                        }
                    ).size(10.0).weak());

                    small_btn(ui, "\u{25b6}").clicked().then(|| {
                        if let Some(idx) = current_idx {
                            if idx + 1 < app.model_list.len() { let _ = app.begin_switch(idx + 1); }
                        }
                    });

                    ui.add_space(3.0);

                    small_btn(ui, "\u{21ba}").clicked().then(|| {
                        if app.is_v2 {
                            app.v2_scale = 1.0;
                            if let Some(v2) = app.v2_model.as_mut() {
                                v2.set_scale(1.0);
                                v2.set_offset(0.0, 0.0);
                            }
                        } else {
                            app.camera.reset_pan();
                        }
                    });
                    small_btn(ui, "+").clicked().then(|| {
                        if app.is_v2 {
                            app.v2_scale = (app.v2_scale * 1.1).min(10.0);
                            if let Some(v2) = app.v2_model.as_mut() {
                                v2.set_scale(app.v2_scale);
                            }
                        } else {
                            app.camera.zoom_in();
                        }
                    });
                    small_btn(ui, "-").clicked().then(|| {
                        if app.is_v2 {
                            app.v2_scale = (app.v2_scale * 0.9).max(0.1);
                            if let Some(v2) = app.v2_model.as_mut() {
                                v2.set_scale(app.v2_scale);
                            }
                        } else {
                            app.camera.zoom_out();
                        }
                    });

                    ui.add_space(3.0);

                    if ui.add(egui::Button::new("\u{2193}").min_size(egui::vec2(30.0, 22.0))).on_hover_text("Minimize to tray").clicked() {
                        app.request_minimize = true;
                    }

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
