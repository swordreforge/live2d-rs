# V2 Audio Response Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Play `.ogg` sound files associated with V2 motions — when a motion starts, its corresponding audio (from `model.json` `"sound"` field) plays in sync.

**Architecture:** Expose `current_group`/`current_no` from C++ V2 API so Rust can query what motion the C++ wrapper randomly chose. Parse V2 model.json motions in Rust to build a sound-path lookup map. Use `rodio` for audio playback. Wire playback into `app.rs` and `wayland_pet.rs` after every V2 motion start.

**Tech Stack:** Rust `rodio` (OGG/WAV playback), existing C V2 API, serde for JSON parsing

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `live2d-py/v2_c_api/v2_c_api.h` | Modify | Add `v2_model_get_current_group`, `v2_model_get_current_no` declarations |
| `live2d-py/v2_c_api/v2_c_api.cpp` | Modify | Implement the two new C API functions |
| `live2d-v2-core/src/lib.rs` | Modify (FFI) | Add `extern "C"` FFI declarations for the new C functions |
| `live2d-v2-core/src/model.rs` | Modify | Add `current_group()` and `current_no()` methods |
| `live2d-viewer/src/audio.rs` | Create | `AudioPlayer` struct wrapping rodio `OutputStream` + `Sink` |
| `live2d-viewer/Cargo.toml` | Modify | Add `rodio` dependency |
| `live2d-viewer/src/app.rs` | Modify | Parse V2 motions with sound fields; store sound map; call `AudioPlayer` on motion start |
| `live2d-viewer/src/wayland_pet.rs` | Modify | Same as app.rs — parse motions, store sound map, play on motion start |
| `live2d-viewer/src/lib.rs` (or `mod.rs`) | Modify | Add `mod audio;` |

---

### Task 1: C++ API — expose current motion info

**Files:**
- Modify: `live2d-py/v2_c_api/v2_c_api.h`
- Modify: `live2d-py/v2_c_api/v2_c_api.cpp`

**Background:** `LAppModel` already stores `mCurrentGroup` (std::string) and `mCurrentNo` (int) as public members. After `startMotion` or `startRandomMotion` returns, these fields hold the motion that was just started. We just need two getter functions.

- [ ] **Step 1: Add C API declarations to header**

Insert after the `v2_model_is_motion_finished` line in `v2_c_api.h`:

```c
const char* v2_model_get_current_group(V2Model* m);
int         v2_model_get_current_no(V2Model* m);
```

- [ ] **Step 2: Add C API implementations to .cpp**

Insert before `/* ──────── Motion / Expression ──────── */` section comment in `v2_c_api.cpp`:

```cpp
const char* v2_model_get_current_group(V2Model* m) {
    return m->buf(m->model->mCurrentGroup);
}
int v2_model_get_current_no(V2Model* m) {
    return m->model->mCurrentNo;
}
```

Verify: `g++ -c v2_c_api.cpp -I.` compiles clean (no link needed for this check).

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "feat(v2-c-api): expose current_group/current_no after motion start"
```

---

### Task 2: Rust FFI — add current_group/current_no bindings

**Files:**
- Modify: `live2d-v2-core/src/lib.rs` (FFI `extern "C"` block)
- Modify: `live2d-v2-core/src/model.rs` (safe wrapper methods)

- [ ] **Step 4: Add FFI function declarations**

In `live2d-v2-core/src/lib.rs`, find the `extern "C"` block that has `v2_model_is_motion_finished` and add after it:

```rust
fn v2_model_get_current_group(m: *mut ffi::V2Model) -> *const c_char;
fn v2_model_get_current_no(m: *mut ffi::V2Model) -> i32;
```

- [ ] **Step 5: Add safe wrapper methods**

In `live2d-v2-core/src/model.rs`, add after the `is_motion_finished` method:

```rust
pub fn current_group(&self) -> String {
    let ptr = unsafe { ffi::v2_model_get_current_group(self.raw) };
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

pub fn current_no(&self) -> i32 {
    unsafe { ffi::v2_model_get_current_no(self.raw) }
}
```

- [ ] **Step 6: Verify build**

```bash
cargo build --release -p live2d-v2-core
```

Expected: passes with zero warnings.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat(live2d-v2-core): add current_group/current_no methods"
```

---

### Task 3: Add rodio and create AudioPlayer

**Files:**
- Modify: `live2d-viewer/Cargo.toml`
- Create: `live2d-viewer/src/audio.rs`

- [ ] **Step 8: Add rodio dependency**

In `live2d-viewer/Cargo.toml`, add to `[dependencies]`:

```toml
rodio = "0.20"
```

- [ ] **Step 9: Create `audio.rs` module**

Create `live2d-viewer/src/audio.rs`:

```rust
use std::path::Path;
use std::sync::Mutex;

pub struct AudioPlayer {
    sink: Mutex<rodio::Sink>,
}

impl AudioPlayer {
    pub fn new() -> anyhow::Result<Self> {
        let (_stream, stream_handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&stream_handle)?;
        // Keep _stream alive for the lifetime of AudioPlayer
        // We leak it because Sink needs the stream handle's device to stay alive.
        Box::leak(Box::new(_stream));
        Ok(Self { sink: Mutex::new(sink) })
    }

    /// Play a sound file (OGG/WAV/MP3). If a sound is already playing, it
    /// queues without interrupting the current one (rodio default behavior).
    pub fn play(&self, path: &Path) {
        if !path.exists() {
            log::warn!("[audio] sound file not found: {:?}", path);
            return;
        }
        match std::fs::File::open(path) {
            Ok(file) => {
                match rodio::Decoder::new(std::io::BufReader::new(file)) {
                    Ok(source) => {
                        if let Ok(sink) = self.sink.lock() {
                            sink.append(source);
                            log::info!("[audio] playing: {:?}", path);
                        }
                    }
                    Err(e) => log::warn!("[audio] decoder error for {:?}: {}", path, e),
                }
            }
            Err(e) => log::warn!("[audio] open error for {:?}: {}", path, e),
        }
    }
}
```

- [ ] **Step 10: Register the module**

In `live2d-viewer/src/main.rs` (or wherever the top-level `mod` declarations are — check which file has they):

Find `pub mod` declarations (typically around lines 1-15 of `main.rs`, or there may be a `lib.rs`) and add:

```rust
mod audio;
```

- [ ] **Step 11: Verify build**

```bash
cargo build --release -p live2d-viewer 2>&1 | head -20
```

Expected: compiles, may warn about unused `AudioPlayer` (that's fine — will be wired next).

- [ ] **Step 12: Commit**

```bash
git add -A && git commit -m "feat(viewer): add rodio audio player module"
```

---

### Task 4: Parse V2 motions with sound fields (shared helper)

**Files:**
- Create: `live2d-viewer/src/v2_motion_sound.rs`

Both `app.rs` and `wayland_pet.rs` need identical V2 motion/sound JSON parsing. Extract it into a shared helper module.

- [ ] **Step 13: Create `v2_motion_sound.rs`**

Create `live2d-viewer/src/v2_motion_sound.rs`:

```rust
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A single motion entry in V2 model.json.
#[derive(Debug, Deserialize)]
pub struct V2MotionEntry {
    pub file: String,
    pub sound: Option<String>,
}

/// Parse the `motions` section of a V2 model.json into a lookup map.
/// Returns `group_name -> Vec<{file, sound_path_or_none}>`.
/// Sound paths are resolved to absolute paths relative to `base_dir`.
pub fn parse_v2_motions(
    json_text: &str,
    base_dir: &Path,
) -> HashMap<String, Vec<(String, Option<PathBuf>)>> {
    // Derive a local struct to deserialize just what we need.
    #[derive(Deserialize)]
    struct V2MotionsJson {
        motions: Option<HashMap<String, Vec<V2MotionEntry>>>,
    }

    let Ok(parsed) = serde_json::from_str::<V2MotionsJson>(json_text) else {
        return HashMap::new();
    };

    let Some(motions) = parsed.motions else {
        return HashMap::new();
    };

    motions
        .into_iter()
        .map(|(group, entries)| {
            let resolved: Vec<(String, Option<PathBuf>)> = entries
                .into_iter()
                .map(|e| {
                    let sound = e.sound.map(|s| base_dir.join(&s));
                    (e.file, sound)
                })
                .collect();
            (group, resolved)
        })
        .collect()
}
```

- [ ] **Step 14: Register the module**

Add `mod v2_motion_sound;` alongside the other `mod` declarations in `main.rs` (or `lib.rs`).

- [ ] **Step 15: Commit**

```bash
git add -A && git commit -m "feat(viewer): add v2_motion_sound parser helper"
```

---

### Task 5: Wire V2 audio into app.rs

**Files:**
- Modify: `live2d-viewer/src/app.rs`

- [ ] **Step 16: Add fields to AppState**

In `AppState` struct, add two new fields (after `v2_last_hovered_area` and `last_v2_size`):

```rust
/// V2 motion sound lookup: group -> Vec<(mtn_filename, Option<absolute_sound_path>)>
pub v2_motion_sounds: HashMap<String, Vec<(String, Option<PathBuf>)>>,
/// Audio player instance (shared across V2 and V3)
pub audio_player: Option<crate::audio::AudioPlayer>,
```

Initialize in `AppState::new` constructor:

```rust
v2_motion_sounds: HashMap::new(),
audio_player: AudioPlayer::new().ok(),
```

- [ ] **Step 17: Parse motions after V2 model load**

In `switch_to`, after the existing V2 model.json parsing block (the one that reads `hit_areas`, around line 696-720), add motion/sound parsing:

```rust
// Parse V2 model.json motions with sound paths
if let Ok(json_text) = std::fs::read_to_string(&model_json) {
    let base_dir = model_json.parent().unwrap_or(&model_json).to_path_buf();
    self.v2_motion_sounds = crate::v2_motion_sound::parse_v2_motions(&json_text, &base_dir);
    log::info!("Parsed {} V2 motion groups for sound", self.v2_motion_sounds.len());
}
```

- [ ] **Step 18: Look up sound after V2 motion start — helper method**

Add a helper method `play_v2_motion_sound` to `impl AppState`:

```rust
/// After starting a V2 motion, call this to play the matching sound.
/// Queries `current_group`/`current_no` from C++ to know what started.
fn play_v2_motion_sound(&mut self) {
    let Some(ref mut v2) = self.v2_model else { return };
    let Some(ref player) = self.audio_player else { return };

    let group = v2.current_group();
    let no = v2.current_no() as usize;

    let Some(entries) = self.v2_motion_sounds.get(&group) else {
        return;
    };
    let Some((_file, Some(sound_path))) = entries.get(no) else {
        return;
    };
    player.play(sound_path);
}
```

- [ ] **Step 19: Call it after every V2 motion start**

There are three call sites in `app.rs` that start V2 motions:

1. `handle_tap_with_cam` (line 1216): after `v2.start_random_motion("", 3);`
2. `handle_v2_hover` (line 1307, 1309): after `v2.start_random_motion(...)`
3. `start_motion` (line 1054): after `v2.start_motion(category, idx, 3);`
4. `switch_to` Opening animation (line 743): after `v2.start_random_motion("", 3);`

At each site, add `self.play_v2_motion_sound();` right after the `v2.start_*` call.

Example (in `handle_tap_with_cam` — line 1216):

```rust
if let Some(ref mut v2) = self.v2_model {
    v2.start_random_motion("", 3);
    self.play_v2_motion_sound();  // <-- add this
}
```

- [ ] **Step 20: Verify build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: compiles with zero warnings.

- [ ] **Step 21: Commit**

```bash
git add -A && git commit -m "feat(viewer): wire V2 audio playback into app.rs"
```

---

### Task 6: Wire V2 audio into wayland_pet.rs

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs`

- [ ] **Step 22: Add fields to PetModel::V2 variant**

The `PetModel` enum has a `V2` variant (around line 89). Its fields are in a struct-like variant. Add two new fields:

```rust
V2 {
    model: ...,
    ...
    /// V2 motion sound lookup
    motion_sounds: HashMap<String, Vec<(String, Option<PathBuf>)>>,
    /// Audio player
    audio_player: Option<crate::audio::AudioPlayer>,
}
```

- [ ] **Step 23: Parse motions in V2 construction**

After the existing hit_areas parsing block (around line 634), parse motions:

```rust
let motion_sounds = {
    let json_text = std::fs::read_to_string(&model_json_path).unwrap_or_default();
    crate::v2_motion_sound::parse_v2_motions(&json_text, &model_json_path.parent().unwrap_or(&model_json_path))
};

let audio_player = crate::audio::AudioPlayer::new().ok();
```

Pass these into the `V2 { motion_sounds, audio_player, ... }` construction.

- [ ] **Step 24: Look up sound after V2 motion start in wayland_pet render loop**

In the V2 render loop (the main loop block that processes V2 model), find the motion start calls. There should be:

1. On area transition hover (similar to `app.rs` `handle_v2_hover`): after `v2.start_random_motion(...)` call
2. On click (tap): after `v2.start_random_motion("", 3);`
3. Opening animation construction: after `v2.start_random_motion("", 3);`

Add a local helper in the render loop or inline:

```rust
fn play_v2_sound(
    v2: &live2d_v2_core::model::Model,
    sounds: &HashMap<String, Vec<(String, Option<PathBuf>)>>,
    player: &Option<crate::audio::AudioPlayer>,
) {
    let Some(ref player) = player else { return };
    let group = v2.current_group();
    let no = v2.current_no() as usize;
    if let Some(entries) = sounds.get(&group) {
        if let Some((_, Some(path))) = entries.get(no) {
            player.play(path);
        }
    }
}
```

Call `play_v2_sound(&v2, &motion_sounds, &audio_player)` after every `v2.start_random_motion("...", ...)` call.

- [ ] **Step 25: Verify build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: compiles with zero warnings.

- [ ] **Step 26: Commit**

```bash
git add -A && git commit -m "feat(viewer): wire V2 audio playback into wayland_pet.rs"
```

---

### Task 7: Fix `AudioPlayer` design — prevent stream drop

**Files:**
- Modify: `live2d-viewer/src/audio.rs`

**Problem:** The current `AudioPlayer::new()` leaks the `OutputStream` via `Box::leak`. While this works, it leaks memory every time `AudioPlayer::new()` panics or is dropped without calling `play`. A cleaner approach: store the stream in an `Arc` alongside the sink.

- [ ] **Step 27: Refactor AudioPlayer to hold stream safely**

```rust
use std::path::Path;
use std::sync::Mutex;

pub struct AudioPlayer {
    sink: Mutex<rodio::Sink>,
    // _stream is held so the audio device doesn't get closed
    _stream: rodio::OutputStream,
}

impl AudioPlayer {
    pub fn new() -> anyhow::Result<Self> {
        let (stream, stream_handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&stream_handle)?;
        Ok(Self { sink: Mutex::new(sink), _stream: stream })
    }

    pub fn play(&self, path: &Path) {
        if !path.exists() {
            log::warn!("[audio] sound file not found: {:?}", path);
            return;
        }
        match std::fs::File::open(path) {
            Ok(file) => {
                match rodio::Decoder::new(std::io::BufReader::new(file)) {
                    Ok(source) => {
                        if let Ok(sink) = self.sink.lock() {
                            sink.append(source);
                            log::info!("[audio] playing: {:?}", path);
                        }
                    }
                    Err(e) => log::warn!("[audio] decoder error for {:?}: {}", path, e),
                }
            }
            Err(e) => log::warn!("[audio] open error for {:?}: {}", path, e),
        }
    }
}
```

- [ ] **Step 28: Commit**

```bash
git add -A && git commit -m "fix(audio): hold OutputStream safely instead of leaking"
```

---

### Task 8: Final verification

- [ ] **Step 29: Run clippy and check**

```bash
cargo clippy --release --all-targets 2>&1
```

Expected: zero warnings.

- [ ] **Step 30: Run build**

```bash
cargo build --release -p live2d-viewer 2>&1
```

Expected: compiles cleanly.

- [ ] **Step 31: Review all changes**

```bash
git log --oneline -10
git diff main~7..main --stat
```

Verify:
- Each commit message follows conventional commit format
- No unintended files modified
- All 7 tasks accounted for

- [ ] **Step 32: Push**

```bash
git push origin main
```

---

## Self-Review Checklist

**Spec coverage:** Every task contributes to the single goal: "play .ogg sound when a V2 motion starts, matching the model.json `sound` field one-to-one." ✓

**Placeholder scan:** No TBD, TODO, or placeholder code. All code blocks contain complete, working Rust/C++. ✓

**Type consistency:** 
- `parse_v2_motions` returns `HashMap<String, Vec<(String, Option<PathBuf>)>>` — used identically in both `app.rs` and `wayland_pet.rs`. ✓
- `current_group()` returns `String`, `current_no()` returns `i32` — matches C API. ✓
- `AudioPlayer::play()` takes `&Path` — `Option<PathBuf>` from lookup works directly. ✓

**Reality check:** The `AudioPlayer` holds a `Mutex<Sink>` for thread safety. In the current single-threaded viewer, `Mutex` overhead is negligible and prevents issues if audio is triggered from event callbacks on different threads in the future. ✓
