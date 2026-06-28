use std::time::Duration;

use crate::ai::types::{AiConfig, ChatMessage, ChatRole};

/// OpenAI-compatible chat completion client.
///
/// Uses `reqwest::blocking` — the winit event loop is synchronous, so
/// `block_on` over an async client buys nothing for MVP. The blocking client
/// sends the HTTP request on the current thread; the UI shows a `Thinking...`
/// state so the user knows progress is happening.
#[allow(dead_code)]
pub struct AiChatClient {
    http: reqwest::blocking::Client,
}

impl AiChatClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("failed to create HTTP client"),
        }
    }

    /// Serialize the message slice into the OpenAI chat completions request,
    /// POST it to `{base_url}/chat/completions`, and return the assistant's
    /// reply text on success.
    pub fn send(&self, messages: &[ChatMessage], config: &AiConfig) -> Result<String, String> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

        let body = serde_json::json!({
            "model": config.model,
            "messages": messages.iter().map(|m| {
                serde_json::json!({
                    "role": match m.role {
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        ChatRole::System => "system",
                    },
                    "content": m.content,
                })
            }).collect::<Vec<_>>(),
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "stream": false,
        });

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .json(&body)
            .send()
            .map_err(|e| format!("Connection failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().unwrap_or_default();
            return Err(format!("API error {status}: {body_text}"));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| format!("Invalid response JSON: {e}"))?;

        let content: String = json["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| "Empty response from model".to_string())?
            .to_string();

        Ok(content)
    }

    /// Send a minimal "ping" to verify the provider configuration is working.
    pub fn test_connection(&self, config: &AiConfig) -> Result<String, String> {
        let msg = ChatMessage {
            role: ChatRole::User,
            content: "Respond with exactly one word: ok".into(),
            timestamp: 0.0,
        };
        self.send(&[msg], config)
    }
}
