# Visual Model Integration — System Architecture & Implementation Plan

## 1. Current System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        live2d-viewer                         │
│                                                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐ │
│  │ Renderer │  │   GUI    │  │  Motion  │  │ AI Chat     │ │
│  │ (glow)   │  │ (egui)   │  │ System   │  │ System      │ │
│  └──────────┘  └──────────┘  └──────────┘  └──────┬──────┘ │
│                                                    │        │
│  ┌──────────────────────┐               ┌─────────▼──────┐ │
│  │ wlr-screencopy       │               │ AiChatClient    │ │
│  │ (Capture Thread)     │               │ (blocking HTTP) │ │
│  │ → mpsc → AppState    │               │ → OpenAI API    │ │
│  └──────────────────────┘               └────────┬────────┘ │
│                                                  │          │
│  ┌──────────────────────┐               ┌───────▼────────┐ │
│  │ Capture Preview      │               │ Tool Registry  │ │
│  │ (egui sub-window)    │               │ exec_cmd, ...  │ │
│  └──────────────────────┘               └────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**Key data flows already in place:**
- `wlr_screencopy::run()` → `mpsc::Sender<CapturedFrame>` → `AppState::capture_latest_frame`
- `AppState::send_ai_message()` → `AiChatClient::send_stream()` → `AppState::ai_messages`
- `AiConfig` with `model`, `base_url`, `api_key`, `system_prompt`, `context_length`
- `CharacterCard` with `name`, `description`, `personality`, `scenario`, `system_prompt`
- `ToolRegistry` with `exec_cmd`, `read_file`, `list_dir`, `get_env`, `read_proc`

**Missing for vision integration:**
- Frame → base64/image encoding
- Vision-capable API message format (OpenAI `image_url` content blocks)
- Trigger mechanisms (auto-periodic, user button, character-initiated)
- Response routing (text → chat bubble / TTS / expression)

---

## 2. Target Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                    Vision-Enhanced Architecture                   │
│                                                                   │
│  Capture Thread                Main Thread                        │
│  ┌─────────────┐              ┌──────────────────────────────┐   │
│  │ wlr-scr.copy│──mpsc──►    │ AppState                     │   │
│  │ (2880×1620) │              │  ├─ capture_latest_frame     │   │
│  └─────────────┘              │  ├─ vision_snapshot: Option  │   │
│                               │  ├─ vision_triggers: Vec    │   │
│  Vision Trigger Sources       │  └─ ai_messages: Vec        │   │
│  ┌──────────────────┐         │                              │   │
│  │ 🕐 Auto Periodic  │────────►├─ timer: every N seconds     │   │
│  │ 👆 User Tap       │────────►├─ F10 hotkey                │   │
│  │ 💬 Character Init │────────►├─ LLM tool call "look"      │   │
│  │ 🎯 Event Hook     │────────►├─ app focus change, etc.    │   │
│  └──────────────────┘         └──────────────┬───────────────┘   │
│                                              │                    │
│                        ┌─────────────────────▼──────────────────┐ │
│                        │        VisionPipeline                   │ │
│                        │                                         │ │
│                        │  frame → scale(512px) → base64 →        │ │
│                        │  ChatMessage{ role:User,                │ │
│                        │    content:[{type:"image_url",          │ │
│                        │      image_url:{url:"data:image/        │ │
│                        │      jpeg;base64,..."}}] }              │ │
│                        │                                         │ │
│                        │  → AiChatClient.send() → LLM response   │ │
│                        │  → Route: text / TTS / expression /     │ │
│                        │           motion trigger                │ │
│                        └─────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. Trigger Mechanisms

### 3.1 Auto-Periodic Vision Trigger (`VisionTrigger::Periodic`)

```rust
struct VisionTimer {
    interval: Duration,      // e.g., every 30s
    last_trigger: Instant,
    enabled: bool,
}
```

- Timer runs in the main event loop (checks on each frame)
- When fired: takes `capture_latest_frame`, encodes as JPEG base64, sends to AI
- Configurable interval via egui settings
- Default: off (user must explicitly enable)

### 3.2 User Button / Hotkey (`VisionTrigger::Manual`)

- **F10** or dedicated egui button (📷) in top-right corner
- One-shot: takes current frame, sends to AI
- Character responds with observation about what's on screen

### 3.3 Character-Initiated (`VisionTrigger::ToolCall`)

- Character's system prompt includes a `look_at_screen` tool
- When the LLM decides it wants to see, it calls the tool
- Tool executor encodes current frame and returns it as the tool result
- This is the most "natural" interaction — the character chooses when to look

```json
{
  "name": "look_at_screen",
  "description": "Take a screenshot and analyze what's on the user's screen. Use this when the user asks what you see, or when you're curious about what they're doing.",
  "parameters": {
    "type": "object",
    "properties": {},
    "required": []
  }
}
```

### 3.4 Event Hook (`VisionTrigger::Event`)

- Application-level events trigger a vision snapshot:
  - Window focus change
  - User switches to a different model
  - User opens/closes a specific application (via window title detection)

---

## 4. Data Flow: Vision Request → Response

```
Trigger fires
    │
    ▼
┌─────────────────────────────┐
│ 1. Take Snapshot            │
│    frame = capture_latest   │
│    if None → skip           │
└─────────────┬───────────────┘
              │
              ▼
┌─────────────────────────────┐
│ 2. Encode Frame             │
│    scale to max 512px width │
│    encode as JPEG (quality  │
│    70) → base64             │
│    (image crate already     │
│     available)              │
└─────────────┬───────────────┘
              │
              ▼
┌─────────────────────────────┐
│ 3. Build Vision Message     │
│    ChatMessage {            │
│      role: User,            │
│      content: [             │
│        {type: "text",       │
│         text: trigger_prompt│
│        },                   │
│        {type: "image_url",  │
│         image_url: {        │
│           url: "data:image/ │
│             jpeg;base64,.." │
│         }                   │
│        }                    │
│      ]                      │
│    }                        │
└─────────────┬───────────────┘
              │
              ▼
┌─────────────────────────────┐
│ 4. Send to LLM              │
│    AiChatClient.            │
│      send_with_vision()     │
│    → VisionResponse {       │
│        text,                │
│        expression,          │
│        tool_calls           │
│      }                      │
└─────────────┬───────────────┘
              │
              ▼
┌─────────────────────────────┐
│ 5. Route Response           │
│    if text → chat bubble    │
│    if tts_enabled → speak   │
│    if expression → apply    │
│    if tool_calls → execute  │
└─────────────────────────────┘
```

---

## 5. Implementation Plan (4 Phases)

### Phase 1: Vision Pipeline Core (2-3 days)

**Goal**: Single manual snapshot → LLM response → displayed in chat.

#### 1.1 Frame Encoding (`capture/encoder.rs` or `ai/vision.rs`)

```rust
// New module: ai/vision.rs

/// Encode a captured frame as a base64 JPEG for vision API consumption.
pub fn encode_frame_for_vision(frame: &CapturedFrame) -> Option<String> {
    // 1. Convert RGBA raw bytes → image::RgbaImage
    // 2. Scale to max 512px wide (preserve aspect ratio)
    // 3. Encode as JPEG (quality 70)
    // 4. Base64 encode
    // 5. Return "data:image/jpeg;base64,..."
}
```

Dependencies: `image` crate (already in Cargo.toml), `base64` (add to Cargo.toml).

#### 1.2 Vision Client (`ai/client.rs` extension)

```rust
// Add to AiChatClient:

/// Send a chat request with a vision frame attached.
pub fn send_vision(
    &self,
    messages: &[ChatMessage],    // conversation history
    frame_base64: &str,           // encoded frame
    prompt: &str,                 // "What's on my screen?"
    config: &AiConfig,
) -> Result<String, String> {
    // Build message with image_url content block
    // POST to {base_url}/chat/completions
    // Return LLM text response
}
```

#### 1.3 Trigger Integration (`app.rs`)

```rust
// Add to AppState:

/// Take a vision snapshot and send to AI (F10 hotkey).
pub fn trigger_vision_snapshot(&mut self) {
    let frame = match self.capture_latest_frame.take() {
        Some(f) => f,
        None => return,
    };
    let encoded = ai::vision::encode_frame_for_vision(&frame);
    // Spawn background thread to send to LLM
    // Store response in ai_messages
}
```

#### 1.4 UI Button (gui.rs)

- Add 📷 button next to 🔴⚪ in top-right corner
- Grayed out when capture is inactive
- Shows spinner while LLM is processing

**Deliverable**: Press F10 → character describes what's on screen in chat.

---

### Phase 2: Auto-Periodic Trigger (1-2 days)

**Goal**: Character periodically looks at screen and comments.

#### 2.1 Vision Config (`ai/config.rs`)

```rust
// Add to AiConfig:
pub struct VisionConfig {
    pub auto_look_enabled: bool,
    pub auto_look_interval_secs: u64,  // default 120
    pub auto_look_prompt: String,      // custom prompt for auto-look
    pub max_image_dimension: u32,      // default 512
    pub jpeg_quality: u8,             // default 70
}
```

#### 2.2 Periodic Timer (`app.rs`)

```rust
// In AppState:
vision_timer: Option<Instant>,

// Called every frame in event loop:
fn tick_vision_timer(&mut self) {
    if !self.ai_config.vision.auto_look_enabled { return; }
    if self.vision_timer.elapsed() < self.vision_interval { return; }
    self.trigger_vision_snapshot("Take a look at what's on screen.");
    self.vision_timer = Some(Instant::now());
}
```

#### 2.3 Settings UI (`ai/settings_panel.rs`)

- Toggle: "Auto Look" on/off
- Slider: interval (30s – 10min)
- Text input: custom auto-look prompt

**Deliverable**: Character spontaneously comments on screen every N seconds.

---

### Phase 3: Character-Initiated Vision (2-3 days)

**Goal**: The LLM itself decides when to look, via tool calling.

#### 3.1 `look_at_screen` Tool (`ai/tools/registry.rs`)

```rust
// Register new tool:
reg.register(
    "look_at_screen",
    "Capture and analyze the user's screen. Use when asked 'what do you see' or when curious about screen content.",
    serde_json::json!({
        "type": "object",
        "properties": {},
        "required": []
    }),
    exec_look_at_screen,
    SafetyLevel::Safe,
);
```

#### 3.2 Tool Executor (`ai/tools/executors.rs`)

```rust
fn exec_look_at_screen(
    _args: &Value,
    _safety: &SafetyConfig,
) -> Result<String, String> {
    // Access AppState::capture_latest_frame
    // Encode as base64 JPEG
    // Return as inline image (or description)
    // For MVP: return "Screen captured: [dimensions], processing..."
    // For full: the LLM's next turn will have the image
}
```

**Challenge**: Tool executor needs access to `capture_latest_frame`, which lives in `AppState`. Options:
1. Pass `Arc<Mutex<CapturedFrame>>` to tool context
2. Send frame as tool result directly (the LLM processes it in the same turn)

#### 3.3 System Prompt Integration

The character card already has a `system_prompt` field. Add vision-awareness:

```
You have access to a `look_at_screen` tool. Use it when the user asks
what you can see, or when you want to comment on what's happening on
their screen. After using it, describe what you see in a natural,
conversational way — as if you're looking over their shoulder.
```

**Deliverable**: Character uses `look_at_screen` tool autonomously when appropriate.

---

### Phase 4: Polish & Production (1-2 days)

#### 4.1 Response Routing

When the LLM responds to a vision query, route appropriately:

```rust
enum VisionResponseAction {
    ChatBubble(String),           // show in chat
    Speak(String),                // TTS voice output
    Expression(String, f32),      // character expression
    Motion(String),               // trigger named motion
    ToolCalls(Vec<ToolCall>),     // execute tools
}
```

#### 4.2 Frame Cache

Avoid re-encoding the same frame:

```rust
struct FrameCache {
    last_frame_hash: u64,
    last_encoded: Option<String>,
}
```

#### 4.3 Error Handling

- Capture not active → message: "Start capture first (F9)"
- Frame too old (>5s) → skip, don't send stale data
- LLM timeout → graceful error in chat

#### 4.4 Event Hooks

```rust
// Example: auto-look when user switches apps
// (detect via window title changes in the capture)
fn on_screen_change(&mut self) {
    if self.ai_config.vision.auto_look_on_change {
        self.trigger_vision_snapshot("Something changed on screen. What happened?");
    }
}
```

---

## 6. Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Image format | JPEG, quality 70 | 512px width → ~30KB, well within API limits |
| Encoding location | Capture thread? Main thread? | **Main thread** (simpler, avoids Send constraints on image crate) |
| Frame access model | `take()` from `capture_latest_frame` | Zero-copy, frame is consumed by vision |
| LLM client | Extend existing `AiChatClient` | Reuse auth, error handling, streaming |
| Tool executor context | `Arc<Mutex<Option<CapturedFrame>>>` | Shared state between event loop and tool executor |
| Character card integration | `system_prompt` field + `look_at_screen` tool | No new config format needed |

## 7. Configuration Schema

```json
{
  "vision": {
    "auto_look_enabled": false,
    "auto_look_interval_secs": 120,
    "auto_look_prompt": "Take a moment to observe what's on the screen. Comment on anything interesting.",
    "max_image_dimension": 512,
    "jpeg_quality": 70
  }
}
```

## 8. File Changes Summary

| Phase | New Files | Modified Files |
|---|---|---|
| 1 | `ai/vision.rs` | `ai/client.rs`, `app.rs`, `gui.rs`, `main.rs` |
| 2 | — | `ai/config.rs`, `ai/settings_panel.rs`, `app.rs` |
| 3 | — | `ai/tools/registry.rs`, `ai/tools/executors.rs` |
| 4 | — | `ai/vision.rs`, `app.rs` |
