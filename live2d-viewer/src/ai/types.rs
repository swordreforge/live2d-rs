use serde::{Deserialize, Serialize};

/// A single message in the AI chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Seconds since epoch (for UI display ordering)
    pub timestamp: f64,
}

/// Role of a chat message sender.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

/// AI provider configuration, persisted to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// The API base URL (e.g. "http://localhost:11434/v1")
    pub base_url: String,
    /// Model name (e.g. "llama3.2", "gpt-4o-mini")
    pub model: String,
    /// API key (may be empty for local models like Ollama)
    pub api_key: String,
    /// Maximum tokens per response
    pub max_tokens: u32,
    /// Temperature for response generation (0.0–2.0)
    pub temperature: f32,
    /// Max recent messages kept in API context
    pub context_length: usize,
    /// System prompt / character card
    pub system_prompt: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            api_key: String::new(),
            max_tokens: 2048,
            temperature: 0.7,
            context_length: 20,
            system_prompt: "You are a cute anime girl living on the user's desktop. \
                           Be friendly, playful, and occasionally cheeky. \
                           Keep responses concise (1-3 sentences)."
                .into(),
        }
    }
}
