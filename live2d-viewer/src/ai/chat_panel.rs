use pulldown_cmark::{Event, Parser, Tag};

use crate::app::AppState;
use crate::ai::types::ChatRole;
use egui::Window;

fn flush_plain(ui: &mut egui::Ui, buf: &mut String, bold: bool) {
    if !buf.is_empty() {
        let text = std::mem::take(buf);
        if bold {
            ui.label(egui::RichText::new(text).strong());
        } else {
            ui.label(text);
        }
    }
}

/// Render a markdown string into egui widgets.
fn render_markdown(ui: &mut egui::Ui, text: &str) {
    let parser = Parser::new(text);
    let mut bold: u32 = 0;
    let mut plain = String::new();

    for event in parser {
        match event {
            Event::Text(t) => plain.push_str(&t),
            Event::Code(t) => {
                flush_plain(ui, &mut plain, bold > 0);
                ui.colored_label(
                    egui::Color32::from_rgb(230, 200, 150),
                    format!(" `{}` ", t),
                );
            }
            Event::SoftBreak | Event::HardBreak => {
                flush_plain(ui, &mut plain, bold > 0);
            }
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::CodeBlock(_kind) => {
                    flush_plain(ui, &mut plain, bold > 0);
                }
                Tag::Emphasis | Tag::Strong => {
                    flush_plain(ui, &mut plain, bold > 0);
                    bold += 1;
                }
                Tag::Heading(..) => {
                    flush_plain(ui, &mut plain, bold > 0);
                    ui.add_space(4.0);
                }
                Tag::List(_) => {
                    flush_plain(ui, &mut plain, bold > 0);
                }
                Tag::Item => {
                    flush_plain(ui, &mut plain, bold > 0);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                Tag::Paragraph => {
                    flush_plain(ui, &mut plain, bold > 0);
                }
                Tag::CodeBlock(_kind) => {}
                Tag::Emphasis | Tag::Strong => {
                    flush_plain(ui, &mut plain, bold > 0);
                    bold = bold.saturating_sub(1);
                }
                Tag::Heading(..) => {
                    flush_plain(ui, &mut plain, bold > 0);
                    ui.add_space(4.0);
                }
                Tag::List(_) => {}
                Tag::Item => {
                    flush_plain(ui, &mut plain, bold > 0);
                }
                _ => {}
            },
            Event::Html(t) => {
                flush_plain(ui, &mut plain, bold > 0);
                ui.label(t.as_ref());
            }
            Event::Rule => {
                flush_plain(ui, &mut plain, bold > 0);
                ui.separator();
            }
            _ => {}
        }
    }
    flush_plain(ui, &mut plain, bold > 0);
}

/// Draw the AI chat panel inside a normal-mode egui window.
pub fn draw_chat_panel(ctx: &egui::Context, app: &mut AppState) {
    if !app.ai_chat_open {
        return;
    }

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

                        if msg.role == ChatRole::Assistant {
                            render_markdown(ui, &msg.content);
                        } else {
                            ui.label(&msg.content);
                        }
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

    if clicked_send && !pending {
        app.send_ai_message();
    } else if enter_triggered && !input_before.trim().is_empty() && !pending {
        app.send_ai_message();
    }
}
