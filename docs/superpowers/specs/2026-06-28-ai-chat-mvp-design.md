# AI Chat MVP — Design Document

Date: 2026-06-28
Status: Draft

## 1. Overview

Add an AI chat companion to `live2d-viewer`, enabling the user to converse with the loaded Live2D character through a text-based chat interface. The AI is powered by an external OpenAI-compatible API endpoint (Ollama, llama.cpp server, OpenAI, Anthropic, etc.). The character's behavior is shaped by a system prompt / "character card."

This is the **MVP** — the minimum viable chat. It intentionally omits memory persistence, context compaction/compression, LLM-driven character autonomy, and bundled inference.

### Principle
**Don't reinvent the wheel.** Use established Rust crates wherever possible — `async-openai` for the API client, `serde` + `dirs` for config persistence (already in deps), `egui` for UI (already in deps). No hand-rolled HTTP, no custom JSON serialization for the API protocol.

### Goals
- User can configure an AI provider (base URL, model name, API key) via a settings panel
- User can type messages and receive responses from the AI
- Chat works in normal window mode (no overlay/pet mode — Wayland complexity excluded from MVP)
- Message context is maintained for the current session (last N messages)
- Character personality is defined by a user-editable system prompt
- Configuration is persisted to disk

### Non-Goals (MVP)
- Memory persistence across restarts (DB storage)
- Context compaction / summarization
- LLM-driven motion/expression control
- Bundled llama.cpp inference
- Voice I/O (ASR/TTS)
- Multiple concurrent conversations
- **Overlay/pet mode chat** — Wayland layer-shell + SCTK subprocess complexity is out of scope for MVP. Chat is normal-window only.

## 2. Architecture

### Module Layout

New module under `live2d-viewer/src/ai/`, gated by a Cargo feature `ai` (default-on):

```
live2d-viewer/src/ai/
  mod.rs              # Module root — re-exports, feature gate
  types.rs            # Shared types: ChatMessage, Role, AiConfig
  client.rs           # AiChatClient wrapping async-openai + tokio runtime
  config.rs           # Config persistence (load/save to JSON on disk)
  chat_panel.rs       # egui chat panel (message list + input)
  settings_panel.rs   # egui settings panel (provider config + character card)
```

### Data Flow

```
User types message in chat_panel
  → AppState.ai_messages.push(ChatMessage { role: User, content: ... })
  → AppState.ai_pending = true
  → client.send(messages)                                          [HTTP POST to /v1/chat/completions]
  → AppState.ai_messages.push(ChatMessage { role: Assistant, content: ... })
  → AppState.ai_pending = false
  → egui re-renders
```

### Integration Points

| Integration | Location | What changes |
|---|---|---|---|
| AppState | `app.rs:212` | Add `ai_messages`, `ai_pending`, `ai_config`, `ai_client`, `ai_input_buffer` fields |
| UI draw dispatch | `gui.rs:draw_normal_ui` | Add chat panel call |
| Settings window | `gui.rs:draw_settings` | Add AI provider config collapsible section |
| Config persistence | `ai/config.rs` (new) | Save/load to `$CONFIG_DIR/ai-config.json` (use `dirs::config_dir()`) |
| Cargo.toml | `live2d-viewer/Cargo.toml` | Add `async-openai` dep, extend `tokio`, add `ai` feature |

## 3. Components

### 3.1 Types (`ai/types.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: f64,  // seconds since epoch (for UI display)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: String,              // "openai-compatible"
    pub base_url: String,              // e.g. "http://localhost:11434/v1"
    pub model: String,                 // e.g. "llama3.2", "gpt-4o"
    pub api_key: String,               // may be empty for local models
    pub max_tokens: u32,               // default 2048
    pub temperature: f32,              // default 0.7
    pub context_length: usize,         // max messages kept in context (default 20)
    pub system_prompt: String,         // character card / personality definition
}

impl Default for AiConfig { ... }
```

### 3.2 Chat Client (`ai/client.rs`)

Uses **`async-openai`** crate — battle-tested, full OpenAI-compatible API client with proper types, error handling, and auth. Works with any endpoint that implements the OpenAI chat completions schema (Ollama, llama.cpp server, OpenAI, Anthropic, vLLM, etc.).

The winit event loop is synchronous. To bridge: create a `tokio::runtime::Runtime` once at startup and `block_on` calls. `tokio` is already a dependency (`features = ["rt"]`); we add `rt-multi-thread` or use `Runtime::new()`.

```rust
use async_openai::{
    Client as OpenAiClient,
    config::OpenAIConfig,
    types::{
        CreateChatCompletionRequestArgs,
        ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs,
        ChatCompletionRequestAssistantMessageArgs,
    },
};

pub struct AiChatClient {
    runtime: tokio::runtime::Runtime,
}

impl AiChatClient {
    pub fn new() -> Self {
        Self {
            runtime: tokio::runtime::Runtime::new()
                .expect("failed to create tokio runtime for AI client"),
        }
    }

    pub fn send(&self, messages: &[ChatMessage], config: &AiConfig) -> Result<String, String> {
        let config = config.clone();  // clone for async block
        let msgs: Vec<ChatCompletionRequestMessage> = messages.iter().map(|m| {
            match m.role {
                ChatRole::System => ChatCompletionRequestSystemMessageArgs::default()
                    .content(&m.content).build().unwrap().into(),
                ChatRole::User => ChatCompletionRequestUserMessageArgs::default()
                    .content(&m.content).build().unwrap().into(),
                ChatRole::Assistant => ChatCompletionRequestAssistantMessageArgs::default()
                    .content(&m.content).build().unwrap().into(),
            }
        }).collect();

        self.runtime.block_on(async {
            let client = OpenAiClient::with_config(
                OpenAIConfig::new()
                    .with_api_base(config.base_url.trim_end_matches('/'))
                    .with_api_key(config.api_key),
            );

            let request = CreateChatCompletionRequestArgs::default()
                .model(&config.model)
                .messages(msgs)
                .max_tokens(config.max_tokens)
                .temperature(config.temperature)
                .build()
                .map_err(|e| format!("Request build failed: {e}"))?;

            let response = client.chat().create(request).await
                .map_err(|e| format!("API error: {e}"))?;

            response.choices.first()
                .and_then(|c| c.message.content.clone())
                .ok_or_else(|| "Empty response from model".into())
        })
    }
}
```

**Key design decisions:**
- Use `async-openai` instead of raw `reqwest` — gets proper typed API, error types, auth handling, streaming support (future), and battle-tested serialization for free.
- One `tokio::Runtime` created at startup (not per-request). `block_on` is acceptable for user-initiated chat (the frame blocks during the request, but the user sees the `Thinking...` state).
- The `async-openai` `Client` is cheap to construct per-call (it's just a config wrapper — the HTTP connection pool is shared internally). We could cache it, but correctness first.
- **Future**: Move to async + `mpsc` channel to avoid blocking the frame loop entirely (e.g., spawn a thread, send results back).

**Error handling** (async-openai provides rich error types — `ApiError`, `HttpError`, `JsonError`, etc.):
- Connection refused → mapped to `"API error: connection refused"`
- Auth failure (401) → `"API error: 401 Unauthorized — check your API key"`
- Model not found (404) → `"API error: 404 — model '{name}' not found"`
- Generic → `"API error: {status_code} — {body}"`
- Display errors in the chat panel as a red system message

### 3.3 Config Persistence (`ai/config.rs`)

```rust
pub fn load_config() -> AiConfig  // returns Default if file doesn't exist
pub fn save_config(config: &AiConfig)
```

- File path: `$CONFIG_DIR/live2d-viewer/ai-config.json`
- Uses `dirs::config_dir()` which is already a dependency
- Atomic write: write to temp file, then rename
- Serialization: `serde_json` (already a dependency)

### 3.4 AppState Changes (`app.rs`)

Add to `AppState`:

```rust
pub struct AppState {
    // ... existing fields ...

    // ── AI Chat ──
    pub ai_enabled: bool,                      // chat panel toggle
    pub ai_messages: Vec<ChatMessage>,         // current conversation
    pub ai_pending: bool,                      // waiting for response
    pub ai_error: Option<String>,              // last error to display
    pub ai_config: AiConfig,                   // loaded from disk
    pub ai_client: AiChatClient,                  // wraps async-openai + tokio runtime
    pub ai_input_buffer: String,               // text input field buffer
}
```

Add method:

```rust
impl AppState {
    pub fn send_ai_message(&mut self) {
        let text = self.ai_input_buffer.trim().to_string();
        if text.is_empty() || self.ai_pending { return; }
        self.ai_input_buffer.clear();

        self.ai_messages.push(ChatMessage {
            role: ChatRole::User,
            content: text,
            timestamp: now(),
        });

        // Build message list for API (system prompt + recent context)
        let mut api_messages = Vec::new();
        if !self.ai_config.system_prompt.is_empty() {
            api_messages.push(ChatMessage {
                role: ChatRole::System,
                content: self.ai_config.system_prompt.clone(),
                timestamp: 0.0,
            });
        }
        let start = self.ai_messages.len().saturating_sub(self.ai_config.context_length);
        api_messages.extend_from_slice(&self.ai_messages[start..]);

        self.ai_pending = true;
        match self.ai_client.send(&api_messages, &self.ai_config) {
            Ok(response) => {
                self.ai_messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: response,
                    timestamp: now(),
                });
                self.ai_error = None;
            }
            Err(e) => {
                self.ai_error = Some(e);
            }
        }
        self.ai_pending = false;
    }
}
```

### 3.5 Chat Panel UI (`ai/chat_panel.rs`)

An egui window/panel showing:

1. **Message list** — scrollable, newest at bottom, auto-scroll on new message
   - Each message rendered with role prefix: `You: ...` / `AI: ...`
   - Different background tint for user vs assistant messages
   - Pending state: show `Thinking...` with animated dots
   - Error state: show red error message inline
2. **Text input** — single-line `TextEdit` at bottom + Send button
   - Send on Enter (without Shift), Shift+Enter for newline
   - Button disabled while pending
3. **Header** — model name + status indicator (online/offline based on last request)

```rust
pub fn draw_chat_panel(ui: &mut egui::Ui, app: &mut AppState)
```

Rendered inside `draw_normal_ui` as a `Window` with `default_width(320.0)`, collapsible, positioned on the right side of the screen.

### 3.6 Settings Panel (`ai/settings_panel.rs`)

An egui collapsible section in the existing settings window:

```
AI Provider
├── Base URL:     [________________________]  (text input)
├── Model:        [________________________]  (text input)
├── API Key:      [________________________]  (password input, optional)
├── Max Tokens:   [256]─────[2048]────────[8192]  (slider)
├── Temperature:  [0.0]──[0.7]──[1.0]─[2.0]  (slider)
├── Context Size: [5]──[20]──[100]  (slider, number of recent messages)
└── [Test Connection]  (button, sends a simple request to validate config)

Character Card (System Prompt)
└── [multiline text edit area]
    You are a cute anime girl living on the desktop...
```

- All changes auto-save to disk (debounced, 500ms after last edit)
- "Test Connection" sends a minimal request (`{"messages":[{"role":"user","content":"ping"}]}`) and shows success/failure feedback

## 4. UI Integration

### Normal Window (`draw_normal_ui`)

- Chat panel appears as a `Window::new("AI Chat")` with `default_width(320.0)`
- Can be toggled from a button near the settings gear
- Default position: right side of the screen, below the settings button area
- Collapsible, remembers its state per session

### Scope note on overlay/pet mode

Chat is implemented only for the normal windowed mode. The AlwaysOnTop overlay and Windowed Pet modes use a separate wayland/sctk subprocess on Linux and a different rendering path that would require significant unsafe/thread-unsafe handling. Excluded from MVP.

## 5. Configuration File

Saved to `$CONFIG_DIR/live2d-viewer/ai-config.json`:

```json
{
  "provider": "openai-compatible",
  "base_url": "http://localhost:11434/v1",
  "model": "llama3.2",
  "api_key": "",
  "max_tokens": 2048,
  "temperature": 0.7,
  "context_length": 20,
  "system_prompt": "You are a cute anime girl living on the user's desktop. Your name is Mao. Be friendly, playful, and occasionally cheeky. Keep responses concise (1-3 sentences)."
}
```

## 6. Error Handling

| Scenario | UX |
|---|---|
| Network unreachable | Red system message in chat: `Connection failed: {error}` |
| API returns 4xx/5xx | Red system message: `API error {code}: {details}` |
| Invalid response format | Red system message: `Unexpected response from API` |
| Empty response | Treated as success (model chose not to respond) |
| API key required but missing | `Test Connection` returns specific error; chat shows `API key required` message |
| Config file corrupted | Load returns defaults, error logged to console |

## 7. Dependencies

Add to `live2d-viewer/Cargo.toml`:

```toml
[dependencies]
async-openai = { version = "0.27", default-features = false, features = ["stream"] }
tokio = { version = "1", default-features = false, features = ["rt", "macros"] }

[features]
default = ["static-link", "ai"]
ai = []
```

- **`async-openai`**: Full OpenAI-compatible API client. Handles serialization, auth, error types, and streaming (for future use) — no hand-rolled HTTP or JSON.
- **`tokio`**: Already a dependency (`features = ["rt"]`). We add `"macros"` for convenience; the runtime is created manually via `tokio::runtime::Runtime::new()`.
- The `ai` feature allows users to opt out if they don't want the chat feature or its dependency tree.
- **No `reqwest` added explicitly** — `async-openai` already depends on it internally.

## 8. File-by-File Change Summary

| File | Change |
|---|---|
| `live2d-viewer/Cargo.toml` | Add `async-openai` dep, extend `tokio`, `ai` feature |
| `live2d-viewer/src/ai/mod.rs` | New — module root, feature gate |
| `live2d-viewer/src/ai/types.rs` | New — `ChatMessage`, `ChatRole`, `AiConfig` (serde) |
| `live2d-viewer/src/ai/client.rs` | New — `AiChatClient` wrapping `async-openai` + tokio runtime |
| `live2d-viewer/src/ai/config.rs` | New — `load_config()` / `save_config()` |
| `live2d-viewer/src/ai/chat_panel.rs` | New — egui chat panel |
| `live2d-viewer/src/ai/settings_panel.rs` | New — egui AI config form |
| `live2d-viewer/src/lib.rs` (or `main.rs`) | Add `mod ai` |
| `live2d-viewer/src/app.rs` | Add AI fields to `AppState` + `AppState::new()` + `send_ai_message()` |
| `live2d-viewer/src/gui.rs` | Add chat panel call in `draw_normal_ui`, AI config in `draw_settings` |

## 9. Future Phases (Post-MVP)

These are explicitly scoped out of MVP but documented here for architectural awareness:

| Phase | Feature | Design Impact |
|---|---|---|
| 2 | Bundled llama.cpp (`llama-cpp-2` crate) | New `BuiltinClient` struct wrapping llama-cpp-2; same `send()` signature |
| 3 | Memory persistence (SQLite via `libsql`) | Store `ai_messages` to DB on each exchange; load last session on startup |
| 4 | Context compaction | `compact_conversation()` — keep recent N turns verbatim, summarize older ones via LLM |
| 5 | Character autonomy | LLM drives motion/expression selection via structured output (JSON mode) |
| 6 | Voice I/O | ASR integration (input) + TTS integration (output) |
