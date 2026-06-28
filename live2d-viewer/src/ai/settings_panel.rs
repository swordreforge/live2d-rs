use crate::app::AppState;
use egui::Window;

/// Draw the AI settings panel (provider config).
pub fn draw_settings_panel(ctx: &egui::Context, app: &mut AppState) {
    if !app.ai_settings_open {
        return;
    }

    Window::new("AI 设置")
        .default_width(360.0)
        .open(&mut app.ai_settings_open)
        .show(ctx, |ui| {
            let dirty = &mut false;

            ui.label("API 基础地址");
            ui.text_edit_singleline(&mut app.ai_config.base_url);

            ui.label("API Key");
            ui.text_edit_singleline(&mut app.ai_config.api_key);

            ui.label("模型");
            ui.text_edit_singleline(&mut app.ai_config.model);

            ui.add_space(8.0);

            ui.label("上下文长度");
            if ui
                .add(egui::DragValue::new(&mut app.ai_config.context_length).clamp_range(1..=100))
                .changed()
            {
                *dirty = true;
            }

            ui.add_space(8.0);

            ui.label("系统提示词 / 角色设定");
            ui.text_edit_multiline(&mut app.ai_config.system_prompt);

            ui.add_space(12.0);

            // Test connection button
            if ui.button("测试连接").clicked() {
                let client = crate::ai::client::AiChatClient::new();
                match client.test_connection(&app.ai_config) {
                    Ok(msg) => {
                        app.ai_test_result = Some((msg, true));
                    }
                    Err(e) => {
                        app.ai_test_result = Some((e, false));
                    }
                }
            }
            if let Some((ref msg, ok)) = app.ai_test_result {
                let color = if ok {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(color, msg);
                if ui.button("清除").clicked() {
                    app.ai_test_result = None;
                }
            }

            ui.add_space(8.0);

            // Save button
            if ui.button("保存设置").clicked() {
                crate::ai::config::save_config(&app.ai_config);
            }
        });
}
