use crate::app::AppState;
use crate::ai::types::ChatRole;
use egui::Window;

/// Draw the AI chat panel inside a normal-mode egui window.
///
/// Because `AppState` is borrowed mutably by the egui closure tree, the
/// actual `send_ai_message()` call is deferred to after the Window closes.
/// Only UI state reads happen inside the closure.
pub fn draw_chat_panel(ctx: &egui::Context, app: &mut AppState) {
    if !app.ai_chat_open {
        return;
    }

    // Snapshot flags before the UI closure to avoid nested borrow conflicts.
    let enter_triggered = app.ai_chat_open
        && ctx.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
    let input_before = app.ai_input_buffer.clone();
    let pending = app.ai_pending;

    let mut clicked_send = false;

    Window::new("AI 聊天")
        .default_width(320.0)
        .default_pos([4.0, 100.0])
        .open(&mut app.ai_chat_open)
        .show(ctx, |ui| {
            ui.label(format!("模型: {}", app.ai_config.model));
            ui.separator();

            let height = ui.available_height() - 60.0;
            egui::ScrollArea::vertical()
                .max_height(height.max(100.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for msg in &app.ai_messages {
                        let (prefix, color) = match msg.role {
                            ChatRole::User => ("你", egui::Color32::LIGHT_BLUE),
                            ChatRole::Assistant => ("AI", egui::Color32::LIGHT_GREEN),
                            ChatRole::System => ("系统", egui::Color32::GRAY),
                        };
                        ui.colored_label(color, format!("[{}]", prefix));
                        ui.label(&msg.content);
                        ui.add_space(4.0);
                    }

                    if pending {
                        ui.colored_label(egui::Color32::YELLOW, "思考中...");
                    }

                    if let Some(ref err) = app.ai_error {
                        ui.colored_label(egui::Color32::RED, err);
                        if ui.button("清除错误").clicked() {
                            app.ai_error = None;
                        }
                    }
                });

            ui.separator();

            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(ui.available_width() - 60.0, 0.0),
                    egui::TextEdit::singleline(&mut app.ai_input_buffer)
                        .hint_text("输入消息...")
                        .desired_width(f32::INFINITY),
                );
                if ui.add_enabled(!pending, egui::Button::new("发送")).clicked() {
                    clicked_send = true;
                }
            });
        });

    // Deferred send after the Window closure releases the borrow.
    if clicked_send && !pending {
        app.send_ai_message();
    } else if enter_triggered && !input_before.trim().is_empty() && !pending {
        app.send_ai_message();
    }
}
