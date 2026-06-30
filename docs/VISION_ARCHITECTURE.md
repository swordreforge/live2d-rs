# Visual Model Integration - Local-First Architecture

## 1. Problem Statement

Extend the existing AI chat assistant with **local screen vision** using llama-cpp-rs.
Primary inference runs locally from GGUF models in `model/`. Remote API (OpenAI/Ollama)
serves as fallback when no local model is loaded.

**What we gain:**
- Privacy: screenshots stay on device by default
- Zero cost for local inference
- Works offline (local models only)
- Seamless integration with existing `ai_messages` chat UI

---

## 2. Target Architecture

```
live2d-viewer

Capture Thread                  Main Thread
+-------------------+         +-------------------------------------+
| wlr-screencopy    |--mpsc-->| AppState                            |
| (2880x1620)       |         |  +- capture_latest_frame            |
+-------------------+         |  +- ai_messages: Vec<ChatMessage>   |
                              |  +- ai_result_rx                    |
                              |  +- model_manager: ModelManager     |
                              +------------------+------------------+
                                                 |
                              +------------------v------------------+
                              |     InferenceBackend (enum)          |
                              |  +----------+    +---------------+  |
                              |  |  Local   |    | Remote (API)  |  |
                              |  | llama-cpp|    | reqwest HTTP  |  |
                              |  +----+-----+    +-------+-------+  |
                              |       |                  |           |
                              |       v                  v           |
                              |   AiStreamEvent (Token/Done/Error)  |
                              |   -> push to ai_messages            |
                              +-------------------------------------+

model/
+-- text/
|   +-- qwen2.5-7b-instruct-q4_k_m.gguf
|   +-- config.json
+-- vision/
|   +-- MiniCPM-V-2_6-Q4_K_M.gguf
|   +-- config.json
+-- global_config.json
```

### Data flow for screen vision

```
Trigger fires (F10 / auto-timer / tool call)
    |
    v
1. Take Snapshot: capture_latest_frame.take()
   if None -> skip
    |
    v
2. Encode: CapturedFrame(RGBA) -> VisionEncoder -> VisionImage(RGB, 336px max)
    |
    v
3. Build prompt: per-model chat template with image placeholder
    |
    v
4. LocalInference::inference_vision(messages, image)
   -> ModelManager.get_vision_session()
   -> llama-cpp-rs processes image + text tokens
   -> sends AiStreamEvent::Token through mpsc channel
    |
    v
5. Same as text chat: tokens accumulate -> AiStreamEvent::Done
   -> push ChatMessage{role: Assistant} into ai_messages
   -> renders in existing chat panel + optional TTS/expression
```

Output is identical to the existing text chat flow -- ChatMessage +
AiStreamEvent channel -> ai_messages -> chat panel UI. No new UI needed.

---

## 3. Model Directory Structure

```
model/
+-- text/                              # Text inference models
|   +-- *.gguf                         # GGUF format model files
|   +-- config.json                    # Per-model config
+-- vision/                            # Vision-language models
|   +-- *.gguf                         # MiniCPM-V / LLaVA / Qwen-VL etc.
|   +-- config.json                    # Per-model config
+-- global_config.json                 # Global AI config
```

### 3.1 Model Config Schema

**model/text/config.json**

    {
      "name": "Qwen2.5-7B-Instruct",
      "file": "qwen2.5-7b-instruct-q4_k_m.gguf",
      "type": "text",
      "n_ctx": 8192,
      "n_gpu_layers": 35,
      "n_threads": 4,
      "temperature": 0.7,
      "top_p": 0.9,
      "repeat_penalty": 1.1
    }

**model/vision/config.json**

    {
      "name": "MiniCPM-V-2.6",
      "file": "MiniCPM-V-2_6-Q4_K_M.gguf",
      "type": "vision",
      "n_ctx": 4096,
      "n_gpu_layers": 35,
      "n_threads": 4,
      "image_resolution": 336,
      "patch_size": 14
    }

**model/global_config.json**

    {
      "active_text_model": "qwen2.5-7b-instruct-q4_k_m.gguf",
      "active_vision_model": "MiniCPM-V-2_6-Q4_K_M.gguf",
      "auto_load": true,
      "max_concurrent_inference": 1,
      "vision": {
        "auto_look_enabled": false,
        "auto_look_interval_secs": 30,
        "auto_look_prompt": "Describe what you see on the screen.",
        "max_image_dimension": 336,
        "jpeg_quality": 85
      }
    }

### 3.2 Recommended Models

| Type | Model | Size (Q4) | VRAM | Notes |
|------|-------|-----------|------|-------|
| Text | Qwen2.5-7B-Instruct-Q4_K_M | ~4.4 GB | ~5 GB | Good Chinese + tool calling |
| Text | Qwen2.5-3B-Instruct-Q4_K_M | ~1.8 GB | ~2.5 GB | Lightweight, faster |
| Vision | MiniCPM-V-2.6-Q4_K_M | ~5.0 GB | ~6 GB | Strong screen understanding |
| Vision | Qwen2-VL-7B-Instruct-Q4_K_M | ~4.5 GB | ~5.5 GB | Alternative vision model |


---

## 4. Core Components

### 4.1 ModelManager (`ai/model_manager.rs`)

Owns loaded LlamaModel instances. Lives in AppState (not inside LocalInference).
Scans model/ directory, loads GGUF files with full config parameters.

    pub struct ModelManager {
        base_dir: PathBuf,
        text_model: Option<LlamaModel>,
        vision_model: Option<LlamaModel>,
        text_session: Option<LlamaSession>,
        vision_session: Option<LlamaSession>,
    }

    impl ModelManager {
        pub fn new(base_dir: PathBuf) -> Self;

        /// Load models declared in global_config.json,
        /// reading n_ctx / n_gpu_layers from per-model config.json.
        pub fn auto_load(&mut self) -> Result<(), ModelError>;

        /// Load a specific model by filename.
        /// Reads config.json from the same directory for n_ctx, n_gpu_layers, etc.
        pub fn load_text_model(&mut self, name: &str) -> Result<(), ModelError>;
        pub fn load_vision_model(&mut self, name: &str) -> Result<(), ModelError>;

        pub fn get_text_session(&mut self) -> Result<&mut LlamaSession, ModelError>;
        pub fn get_vision_session(&mut self) -> Result<&mut LlamaSession, ModelError>;

        pub fn has_text_model(&self) -> bool;
        pub fn has_vision_model(&self) -> bool;
        pub fn list_available_models(&self) -> Vec<ModelInfo>;

        /// Unload all models to free GPU memory
        pub fn unload_all(&mut self);
    }

    pub struct ModelInfo {
        pub name: String,
        pub model_type: ModelType,
        pub path: PathBuf,
        pub size_bytes: u64,
    }

    pub enum ModelType { Text, Vision }

    pub enum ModelError {
        ModelNotFound(PathBuf),
        LoadFailed(String),
        ConfigError(String),
        TextModelNotLoaded,
        VisionModelNotLoaded,
    }


### 4.2 InferenceBackend (`ai/backend.rs`)

Unified interface that routes to local llama-cpp-rs or remote API.
Both backends produce the same AiStreamEvent channel output, so AppState
needs no changes to the streaming pipeline.

    pub enum InferenceBackend {
        Local(LocalInference),
        Remote(AiChatClient),
    }

    impl InferenceBackend {
        /// Create: prefer local if model/ has .gguf, else remote API.
        pub fn auto_detect(model_manager: &mut ModelManager, config: &AiConfig) -> Self;

        /// Text chat: send messages, stream tokens via tx.
        pub fn send_stream(
            &mut self,
            messages: &[ChatMessage],
            config: &AiConfig,
            tools: Option<&[ToolDefinition]>,
            tx: Sender<AiStreamEvent>,
        );

        /// Vision: send messages + image, stream tokens via tx.
        pub fn send_vision_stream(
            &mut self,
            messages: &[ChatMessage],
            image: &VisionImage,
            config: &AiConfig,
            tx: Sender<AiStreamEvent>,
        );
    }

For Local variant: calls LocalInference methods directly.
For Remote variant: calls existing AiChatClient.send_stream()
with OpenAI image_url format for vision (base64 JPEG).


### 4.3 LocalInference (`ai/local_inference.rs`)

Borrows ModelManager (does not own it). Formats prompts per model type.
Produces AiStreamEvent tokens through the same channel as remote API.

    pub struct LocalInference {
        model_manager: *mut ModelManager,  // non-owning, safe via single-threaded access
    }

    impl LocalInference {
        pub fn new(model_manager: &mut ModelManager) -> Self;

        /// Text inference, streaming tokens to tx.
        pub fn inference_text_stream(
            &mut self,
            messages: &[ChatMessage],
            config: &AiConfig,
            tx: Sender<AiStreamEvent>,
        );

        /// Vision inference, streaming tokens to tx.
        pub fn inference_vision_stream(
            &mut self,
            messages: &[ChatMessage],
            image: &VisionImage,
            config: &AiConfig,
            tx: Sender<AiStreamEvent>,
        );
    }

Output format: identical to remote API. Sends AiStreamEvent::Token,
AiStreamEvent::ToolCall, AiStreamEvent::Done, AiStreamEvent::Error.
AppState::complete_pending_switch() processes these the same way.


### 4.4 VisionEncoder (`ai/vision_encoder.rs`)

Single source for VisionImage type and frame encoding.
Used by both local and remote backends (remote needs base64 JPEG).

    pub struct VisionImage {
        pub width: u32,
        pub height: u32,
        pub data: Vec<u8>,  // RGB pixels, no alpha
    }

    impl VisionImage {
        /// Encode to base64 JPEG (for Remote backend / OpenAI API).
        pub fn to_base64_jpeg(&self, quality: u8) -> String;
    }

    /// Convert CapturedFrame (RGBA) to VisionImage (RGB, resized).
    pub fn encode_frame_for_vision(
        frame: &CapturedFrame,
        max_dimension: u32,
    ) -> Result<VisionImage, EncodingError> {
        // 1. RGBA bytes -> RgbaImage
        // 2. Resize so longest side = max_dimension (preserve aspect ratio)
        // 3. Convert RGBA -> RGB (drop alpha channel)
        // 4. Return VisionImage { width, height, data }
    }

Local backend passes raw RGB to llama-cpp-rs.
Remote backend calls vision_image.to_base64_jpeg() for the API.


### 4.5 Prompt Formatting (`ai/prompt_format.rs`)

Different models expect different chat templates.
The prompt formatter reads model type from config.json.

    trait PromptFormatter {
        fn format_chat(&self, messages: &[ChatMessage], has_image: bool) -> String;
    }

    struct MiniCPMVFormatter;   // image + system/user tags
    struct LLaVAFormatter;      // USER/ASSISTANT roles with image token
    struct ChatMLFormatter;     // for Qwen text models (im_start/im_end)

The exact template must match the GGUF model card.


---

## 5. Trigger Mechanisms

### 5.1 Auto-Periodic Vision Trigger

Timer runs in the main event loop (checks on each frame).
When fired: takes capture_latest_frame, encodes as RGB, sends to vision model.
Configurable interval via egui settings. Default: off.

    struct VisionTimer {
        interval: Duration,      // default: 30s (matches global_config.json)
        last_trigger: Instant,
        enabled: bool,
    }

### 5.2 User Hotkey (F10)

One-shot: takes current frame, sends to vision model.
Character responds with observation about what is on screen.

### 5.3 Character-Initiated (Tool Call)

Flow: text model (e.g. Qwen2.5) calls look_at_screen tool
  -> AppState tool executor captures current frame
  -> InferenceBackend.send_vision_stream() with the frame
  -> vision model (e.g. MiniCPM-V) analyzes the image
  -> result pushed to ai_messages as assistant response

The text model decides WHEN to look. The vision model analyzes WHAT it sees.
This requires both a text model AND a vision model loaded simultaneously.
If only text model is loaded, look_at_screen returns a text-only error.

### 5.4 Event Hook

Application-level events trigger a vision snapshot:
- Window focus change
- User switches to a different model
- User opens/closes a specific application


---

## 6. Implementation Plan

### Phase 1: Local Text Inference (2-3 days)

Goal: Local model produces chat responses via same AiStreamEvent channel.

#### 1.1 Add dependencies

    [dependencies]
    llama-cpp-2 = "0.1"

#### 1.2 ModelManager (ai/model_manager.rs)

- Scan model/text/ and model/vision/ for .gguf files
- Read per-model config.json for n_ctx, n_gpu_layers, n_threads
- Load active models from global_config.json
- Create LlamaSession for each loaded model
- GPU layer offloading via n_gpu_layers config

#### 1.3 InferenceBackend (ai/backend.rs)

- InferenceBackend::auto_detect(): if .gguf exists -> Local, else -> Remote
- send_stream(): delegates to LocalInference or AiChatClient
- Same AiStreamEvent output for both paths

#### 1.4 Integration into AppState

- Replace direct AiChatClient usage with InferenceBackend
- AppState::send_ai_message() calls backend.send_stream()
- complete_pending_switch() unchanged (processes AiStreamEvent)

Deliverable: Type a message -> local model responds -> appears in chat panel.

### Phase 2: Vision Pipeline (3-4 days)

Goal: Screen capture -> vision model -> character describes what it sees.
Output goes through same AiStreamEvent -> ai_messages pipeline.

#### 2.1 VisionEncoder (ai/vision_encoder.rs)

- VisionImage struct + encode_frame_for_vision()
- to_base64_jpeg() for remote API fallback
- Use image crate (already in Cargo.toml)

#### 2.2 PromptFormatter (ai/prompt_format.rs)

- PromptFormatter trait with per-model implementations
- MiniCPMVFormatter, LLaVAFormatter, ChatMLFormatter
- Reads model type from config.json to select formatter

#### 2.3 Local Vision Inference

- InferenceBackend::send_vision_stream()
- Local: LocalInference::inference_vision_stream()
- Remote: build OpenAI image_url message, call AiChatClient
- Both produce identical AiStreamEvent output

#### 2.4 Trigger Integration

- F10 hotkey -> take snapshot -> encode -> vision -> display in chat
- VisionTimer for periodic auto-look
- look_at_screen tool for character-initiated vision

Deliverable: Press F10 -> character describes what is on screen.

### Phase 3: Model Management UI (2-3 days)

Goal: User can browse, load, switch models from egui settings panel.

#### 3.1 Model Browser (ai/model_panel.rs)

- List available GGUF files in model/text/ and model/vision/
- Show model name, size, type
- Load/unload button per model
- Active model indicator

#### 3.2 Auto-Download (Optional)

- Download recommended models from HuggingFace on first run
- Progress bar in egui
- Verify checksum after download

#### 3.3 Settings Panel Update

- GPU layers slider (0 = CPU only, max = full GPU offload)
- Context length selector
- Thread count selector
- Temperature / top_p / repeat_penalty sliders
- Backend mode selector: Local / Remote / Auto

Deliverable: User selects model -> model loads -> chat works locally.

### Phase 4: Polish (1-2 days)

#### 4.1 Hybrid Mode

- Local model for text chat (primary)
- Local vision model for screen analysis
- Remote API fallback when no local model is available
- Configurable in settings: which backend for text, which for vision

#### 4.2 Frame Cache

    struct FrameCache {
        last_frame_hash: u64,
        last_encoded: Option<VisionImage>,
    }

#### 4.3 Error Handling

- No model loaded -> InferenceBackend falls back to Remote
- No local vision model -> look_at_screen returns error text
- Model too large for GPU -> fall back to CPU with warning
- Frame too old (>5s) -> skip, do not analyze stale data
- Remote API unreachable -> error message in chat


---

## 7. Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Inference backend | Local-first + Remote fallback | Privacy by default, graceful degradation |
| Model format | GGUF | Standard quantized format, wide model availability |
| Image encoding | Raw RGB (local) / base64 JPEG (remote) | Optimal per backend |
| Image resolution | 336px max | Balances quality vs VRAM usage |
| Model loading | On-demand from config | Avoid loading unused models, save VRAM |
| Prompt format | Per-model PromptFormatter trait | Different models use different chat formats |
| Concurrency | Single inference at a time | Avoids GPU memory contention |
| Output pipeline | Same AiStreamEvent channel | Zero changes to chat UI and message history |
| VisionImage | Single type in vision_encoder.rs | Shared by local (raw RGB) and remote (base64 JPEG) |
| ModelManager | Owned by AppState, borrowed by LocalInference | No ownership conflicts, accessible to UI |
| Text+Vision tool | Text model calls tool -> scheduler triggers vision | Two models collaborate via tool calling |

---

## 8. File Changes Summary

| Phase | New Files | Modified Files |
|---|---|---|
| 1 | ai/model_manager.rs, ai/backend.rs | ai/mod.rs, ai/client.rs, ai/types.rs, Cargo.toml, app.rs |
| 2 | ai/vision_encoder.rs, ai/prompt_format.rs, ai/local_inference.rs | ai/mod.rs, ai/backend.rs, app.rs |
| 3 | ai/model_panel.rs | ai/mod.rs, gui.rs, ai/settings_panel.rs |
| 4 | - | ai/backend.rs, ai/vision_encoder.rs, app.rs, ai/settings_panel.rs |

---

## 9. Model Setup Instructions

1. Create model directory structure:
   mkdir -p model/text model/vision

2. Download text model (e.g., Qwen2.5):
   wget -O model/text/qwen2.5-7b-instruct-q4_k_m.gguf      https://huggingface.co/Qwen/Qwen2.5-7B-Instruct-GGUF/resolve/main/qwen2.5-7b-instruct-q4_k_m.gguf

3. Download vision model (e.g., MiniCPM-V):
   wget -O model/vision/MiniCPM-V-2_6-Q4_K_M.gguf      https://huggingface.co/openbmb/MiniCPM-V-2_6-gguf/resolve/main/MiniCPM-V-2_6-Q4_K_M.gguf

4. Create model/global_config.json with active model names.

5. Run the viewer -- models auto-load on startup.
   If no local model found, falls back to remote API (configure in Settings -> AI).
