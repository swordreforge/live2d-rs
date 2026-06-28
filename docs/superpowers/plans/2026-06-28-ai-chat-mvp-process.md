# AI Chat MVP — Implementation Process

**Principle: Don't reinvent the wheel.** Each component chooses the best existing crate:
- **API client** → `async-openai` (typed, battle-tested, any OpenAI-compatible endpoint)
- **Config persistence** → `serde_json` + `dirs` (already deps)
- **UI** → `egui` (already dep, no alternative)
- **Future: bundled inference** → `` (mature Rust bindings)

Each step is independently testable: compiles → runs → see result. No step depends on later ones.

```
Step 1: Module skeleton + types + config   [cargo check]
    ↓
Step 2: Chat client (HTTP provider)         [talk to Ollama/llama.cpp from CLI]
    ↓
Step 3: AppState integration                [loads config, can send without UI]
    ↓
Step 4: Chat panel (normal window UI)       [chat in normal mode  ✅ MVP DONE]
    ↓
Step 5: Settings panel (provider config UI)  [configure from GUI]
```

---

## Step 1: Module skeleton + types + config

**Files to create/modify:**
- `live2d-viewer/src/ai/mod.rs` — `pub mod types; pub mod config;`
- `live2d-viewer/src/ai/types.rs` — `ChatMessage`, `ChatRole`, `AiConfig` with `Default`
- `live2d-viewer/src/ai/config.rs` — `load_config()`, `save_config()`, file path logic
- `live2d-viewer/src/lib.rs` (or wherever `mod` declarations live) — add `mod ai;` behind `#[cfg(feature = "ai")]`
- `live2d-viewer/Cargo.toml` — add `ai` feature flag (default-on), no new deps yet

**Verification:** `cargo check --release -p live2d-viewer` passes.

**Deliverable:** Module compiles, config reads/writes JSON to disk.

---

## Step 2: Chat client (HTTP provider via `async-openai`)

**Design rationale:** Use the battle-tested [`async-openai`](https://crates.io/crates/async-openai) crate instead of hand-rolling HTTP + JSON serialization with `reqwest`. `async-openai` provides proper typed request/response types, error handling (ApiError, HttpError, JsonError), authentication, and streaming support — all for free. The winit event loop is synchronous, so we create a `tokio::runtime::Runtime` once and use `block_on`.

**Files to create/modify:**
- `live2d-viewer/src/ai/client.rs` — `AiChatClient` struct with `runtime: tokio::runtime::Runtime`, `send()` method that calls `async-openai`'s `chat().create()` via `block_on`
- `live2d-viewer/Cargo.toml` — add `async-openai = { version = "0.27", default-features = false, features = ["stream"] }`, extend `tokio` with `features = ["rt", "macros"]`

**Verification:** Write a test in `client.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_send_basic() {
        let config = AiConfig {
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3.2".into(),
            ..Default::default()
        };
        let client = AiChatClient::new();
        let resp = client.send(&[
            ChatMessage { role: ChatRole::User, content: "Say hello in one word".into(), timestamp: 0.0 }
        ], &config);
        assert!(resp.is_ok());
        println!("{}", resp.unwrap());
    }
}
```
Run with `cargo test --release -p live2d-viewer test_send_basic -- --nocapture` (requires Ollama at localhost:11434). Mark with `#[ignore]` for CI.

**Deliverable:** Can make API calls through a proper typed client — no hand-rolled HTTP.

## Step 2b (alternative, if async-openai is too heavy)

If `async-openai`'s dependency tree is a concern (~60 deps), fall back to raw `reqwest::blocking` + `serde_json` (both already transitive deps). The trade-off is ~20 lines of hand-rolled serialize/deserialize vs. a larger but well-tested crate. Decision deferred to implementation — both paths are compatible with the rest of the design.

---

## Step 3: AppState integration

**Files to modify:**
- `live2d-viewer/src/app.rs` — add fields to `AppState`:
  - `ai_messages: Vec<ChatMessage>`
  - `ai_pending: bool`
  - `ai_error: Option<String>`
  - `ai_config: AiConfig`
  - `ai_client: AiChatClient`
  - `ai_input_buffer: String`
  - `ai_enabled: bool` (default false, toggled from UI button)
- `AppState::new()` — call `ai::config::load_config()`, create client
- Add method `send_ai_message()` on `AppState` (as designed in the spec)
- Wire `drop(app)` / shutdown to call `ai::config::save_config()` on exit

**Verification:**
- `cargo check --release -p live2d-viewer` passes
- On startup, config is loaded (check log output)
- On exit, config is saved (check file timestamp)
- Program runs normally, no visible change (no UI yet)

**Deliverable:** AI subsystem lives and breathes inside the app — just not visible yet.

---

## Step 4: Chat panel (normal window UI)

**Files to create/modify:**
- `live2d-viewer/src/ai/chat_panel.rs` — `draw_chat_panel(ui, app)`:
  - Scrollable message list (auto-scroll to bottom on new msg)
  - Each message: colored role prefix + content
  - `Thinking...` indicator when `ai_pending == true`
  - Error message in red when `ai_error` is set
  - Text input row: `TextEdit` + Send button
  - Send on Enter (not Shift+Enter), clear input buffer
- `live2d-viewer/src/gui.rs` — in `draw_normal_ui`:
  - Add `Window::new("AI Chat")` that shows `chat_panel::draw_chat_panel()`
  - Add a chat toggle button somewhere (e.g. near settings gear)

**Verification:**
- `cargo build --release -p live2d-viewer` compiles
- Run viewer, open AI Chat window, type a message, see response appear
- Messages survive scrolling and window resize
- Error state renders correctly (disconnect network, send message)

**Deliverable:** Chat works in normal window mode.

---

## Step 5: Settings panel (provider config UI)

**Files to create/modify:**
- `live2d-viewer/src/ai/settings_panel.rs` — `draw_ai_settings(ui, app)`:
  - Collapsible "AI Provider" section in settings
  - Fields: base URL, model, API key (password input), max tokens slider, temperature slider, context length slider
  - "Test Connection" button (runs a minimal request, shows success/failure inline)
  - Large multiline text edit for system prompt (character card)
  - Auto-save on every change (debounced / on-field-lost-focus)
- `live2d-viewer/src/gui.rs` — in `draw_settings`, call `ai::settings_panel::draw_ai_settings()`

**Verification:**
- `cargo build --release` compiles
- Open settings, configure provider, change fields, close settings, reopen — values persist
- Test Connection returns success (green checkmark) with real server, error (red X) without
- Character card text persists across restart

**Deliverable:** Full provider configuration from UI.

---

## Step 6: (deferred — overlay mode)

Chat in AlwaysOnTop / Windowed Pet modes is **excluded from MVP** due to Wayland (`smithay-client-toolkit` `layer-shell`) complexity. The pet UI lives in a separate thread with raw GL rendering and an entirely different input dispatch path — adding interactive egui elements there requires tackling thread safety, pointer event routing, and GTK/Wayland compositor quirks. Worth doing, but not in MVP.

---

## Future Steps (not in this process)

| # | Feature | Precondition |
|---|---|---|
| 6 | Overlay/pet mode chat (Wayland) | MVP chat works in normal mode |
| 7 | Bundled llama.cpp inference (`llama-cpp-2` crate) | External API works and is stable |
| 8 | Message persistence via libsql (load last session) | Config persistence pattern is proven |
| 9 | Context compaction (summarize old turns) | Message list can grow large |
| 10 | LLM-driven motion/expression (JSON mode) | Chat pipeline works reliably |
| 11 | Voice input (ASR) + Voice output (TTS) | Chat pipeline is stable |
