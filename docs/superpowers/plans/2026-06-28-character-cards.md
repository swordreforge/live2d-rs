# Character Cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-model character cards (name, description, personality, scenario, example dialogs, system_prompt, tts_voice) stored in SQLite and loaded on model switch.

**Architecture:** New `character_cards` SQLite table keyed by `file_path`; `CharacterCard` struct in `ai/types.rs`; `AppState.current_character_card` loaded after model switch; `send_ai_message()` builds synthetic system prompt from card fields; TTS voice overridable per card.

**Tech Stack:** Rust, libsql (SQLite), egui (UI), serde (JSON serialization)

---

### Task 1: `db.rs` — table migration + 3 query methods

**Files:**
- Modify: `live2d-viewer/src/db.rs`

- [ ] **Step 1: Add `CREATE TABLE IF NOT EXISTS character_cards` to the schema batch in `AppDb::open()`**

Append to the existing `execute_batch` call (around line 110-141). Add after `parameter_presets` table:

```sql
CREATE TABLE IF NOT EXISTS character_cards (
    file_path TEXT PRIMARY KEY REFERENCES model_history(file_path) ON DELETE CASCADE,
    name            TEXT NOT NULL DEFAULT '',
    description     TEXT NOT NULL DEFAULT '',
    personality     TEXT NOT NULL DEFAULT '',
    scenario        TEXT NOT NULL DEFAULT '',
    example_dialogs TEXT NOT NULL DEFAULT '',
    system_prompt   TEXT NOT NULL DEFAULT '',
    tts_voice       TEXT NOT NULL DEFAULT '',
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
```

- [ ] **Step 2: Add `get_character_card()` method** (use full path since CharacterCard is defined in Task 2)

```rust
pub fn get_character_card(&self, file_path: &str) -> Result<Option<crate::ai::types::CharacterCard>> {
    let mut rows = rt().block_on(self.conn.query(
        "SELECT file_path, name, description, personality, scenario, \
         example_dialogs, system_prompt, tts_voice \
         FROM character_cards WHERE file_path = ?1",
        libsql::params![file_path],
    ))?;
    match rt().block_on(rows.next())? {
        Some(row) => Ok(Some(crate::ai::types::CharacterCard {
            file_path: row.get::<String>(0)?,
            name: row.get::<String>(1)?,
            description: row.get::<String>(2)?,
            personality: row.get::<String>(3)?,
            scenario: row.get::<String>(4)?,
            example_dialogs: row.get::<String>(5)?,
            system_prompt: row.get::<String>(6)?,
            tts_voice: row.get::<String>(7)?,
        })),
        None => Ok(None),
    }
}
```

- [ ] **Step 3: Add `save_character_card()` method (upsert)**

```rust
pub fn save_character_card(&self, card: &crate::ai::types::CharacterCard) -> Result<()> {
    rt().block_on(self.conn.execute(
        "INSERT INTO character_cards \
         (file_path, name, description, personality, scenario, \
          example_dialogs, system_prompt, tts_voice) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(file_path) DO UPDATE SET \
         name=?2, description=?3, personality=?4, scenario=?5, \
         example_dialogs=?6, system_prompt=?7, tts_voice=?8, \
         updated_at=datetime('now')",
        libsql::params![
            card.file_path, card.name, card.description, card.personality,
            card.scenario, card.example_dialogs, card.system_prompt, card.tts_voice,
        ],
    ))?;
    Ok(())
}
```

- [ ] **Step 4: Add `delete_character_card()` method**

```rust
pub fn delete_character_card(&self, file_path: &str) -> Result<()> {
    rt().block_on(self.conn.execute(
        "DELETE FROM character_cards WHERE file_path = ?1",
        libsql::params![file_path],
    ))?;
    Ok(())
}
```

- [ ] **Step 5: Update `db.rs` import — add `use crate::ai::types::CharacterCard;`**

Add to the existing `use` block at the top of `db.rs`:

```rust
use crate::ai::types::CharacterCard;
```

(The struct is defined in Task 2, so the import will resolve after both tasks are done. Compilation is verified as part of the final Task 3/7 check.)

- [ ] **Step 6: Commit** (may have compilation failure at this point because CharacterCard isn't defined yet — that's OK, continue to Task 2)

```bash
git add live2d-viewer/src/db.rs
git commit -m "feat(db): add character_cards table and CRUD methods"
```

---

### Task 2: `ai/types.rs` — CharacterCard struct

**Files:**
- Modify: `live2d-viewer/src/ai/types.rs`

- [ ] **Step 1: Add `CharacterCard` struct after the existing `AiConfig` struct**

```rust
/// Per-model character card.
///
/// Stored in the SQLite `character_cards` table keyed by model file_path.
/// All fields are free-text; the system prompt sent to the AI is
/// constructed by concatenating non-empty fields at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterCard {
    pub file_path: String,
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub example_dialogs: String,
    pub system_prompt: String,
    pub tts_voice: String,
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean

- [ ] **Step 3: Commit**

```bash
git add live2d-viewer/src/ai/types.rs
git commit -m "feat(ai): add CharacterCard struct"
```

---

### Task 3: `app.rs` — AppState fields + load_character_card() + prompt construction + TTS override

**Files:**
- Modify: `live2d-viewer/src/app.rs`

- [ ] **Step 1: Add new fields to `AppState`**

Add after the TTS fields (around line 368):

```rust
    // ── Character Card ──
    /// The current model's character card, loaded on model switch.
    pub current_character_card: Option<crate::ai::types::CharacterCard>,
    /// Whether the character card editor window is open.
    pub character_card_editor_open: bool,
```

Initialize in `AppState::new()` (around line 474):

```rust
            current_character_card: None,
            character_card_editor_open: false,
```

- [ ] **Step 2: Add `load_character_card()` method to `impl AppState`**

Add after `refresh_presets()` (around line 667):

```rust
    /// Load the character card for the current model from the database.
    pub fn load_character_card(&mut self) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => {
                self.current_character_card = None;
                return;
            }
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => {
                self.current_character_card = None;
                return;
            }
        };
        self.current_character_card = match self.db {
            Some(ref db) => db.get_character_card(&path).unwrap_or(None),
            None => None,
        };
    }
```

- [ ] **Step 3: Call `load_character_card()` at end of `complete_v3_switch()`**

Add at the end of `complete_v3_switch()` (after the last line, around line 1358+), before the closing `}`:

```rust
        // Load character card for this model
        self.load_character_card();
```

- [ ] **Step 4: Update `send_ai_message()` to use character card for system prompt**

Replace the system prompt block (lines 841-847):

**Old:**
```rust
        let mut api_messages = Vec::new();
        if !self.ai_config.system_prompt.is_empty() {
            api_messages.push(crate::ai::types::ChatMessage {
                role: crate::ai::types::ChatRole::System,
                content: self.ai_config.system_prompt.clone(),
                timestamp: 0.0,
            });
        }
```

**New:**
```rust
        let mut api_messages = Vec::new();
        let system_content = self.build_system_prompt();
        if !system_content.is_empty() {
            api_messages.push(crate::ai::types::ChatMessage {
                role: crate::ai::types::ChatRole::System,
                content: system_content,
                timestamp: 0.0,
            });
        }
```

- [ ] **Step 5: Add `build_system_prompt()` method**

Add a new method to `impl AppState`:

```rust
    /// Build the system prompt for the AI request.
    ///
    /// When a character card is active for the current model, concatenates
    /// its non-empty fields into a synthetic prompt. Otherwise falls back
    /// to the global `ai_config.system_prompt`.
    fn build_system_prompt(&self) -> String {
        let card = match self.current_character_card {
            Some(ref c) => c,
            None => return self.ai_config.system_prompt.clone(),
        };

        let mut parts: Vec<&str> = Vec::new();
        if !card.personality.is_empty() { parts.push(&card.personality); }
        if !card.scenario.is_empty() { parts.push(&card.scenario); }
        if !card.description.is_empty() { parts.push(&card.description); }

        // Example dialogs
        if !card.example_dialogs.is_empty() {
            let name = if card.name.is_empty() { "AI" } else { &card.name };
            // Use a helper to push the labeled section
            let example_section = format!(
                "以下是 {} 的对话示例：\n{}",
                name,
                card.example_dialogs,
            );
            // Build result with all parts
            let mut result = parts.join("\n\n");
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(&example_section);
            if !card.system_prompt.is_empty() {
                result.push_str("\n\n");
                result.push_str(&card.system_prompt);
            }
            return result;
        }

        // No example dialogs
        if card.system_prompt.is_empty() && parts.is_empty() {
            // All card fields empty — fall back to global prompt
            return self.ai_config.system_prompt.clone();
        }

        let mut result = parts.join("\n\n");
        if !card.system_prompt.is_empty() {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(&card.system_prompt);
        }
        result
    }
```

- [ ] **Step 6: Override TTS voice from character card in `poll_ai_result()`**

In the TTS block (around line 917-918), after `let tts_config = self.ai_config.clone();`, add:

```rust
                // Use character card TTS voice if set
                let tts_voice = self.current_character_card
                    .as_ref()
                    .and_then(|c| if c.tts_voice.is_empty() { None } else { Some(c.tts_voice.clone()) })
                    .unwrap_or_else(|| tts_config.tts_voice.clone());
```

Then change the `synthesize` call on line 928 to use `&tts_voice` instead of `&tts_config.tts_voice`.

- [ ] **Step 7: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean, no warnings from new code

- [ ] **Step 8: Commit**

```bash
git add live2d-viewer/src/app.rs
git commit -m "feat(app): character card loading, prompt construction, TTS voice override"
```

---

### Task 4: `ai/character_card_panel.rs` — new editor UI

**Files:**
- Create: `live2d-viewer/src/ai/character_card_panel.rs`

- [ ] **Step 1: Write the editor panel**

```rust
use crate::app::AppState;
use egui::Window;

/// Draw the character card editor window.
pub fn draw_character_card_editor(ctx: &egui::Context, app: &mut AppState) {
    if !app.character_card_editor_open {
        return;
    }

    let card = app.current_character_card.get_or_insert_with(|| {
        // Create a new card for the current model (lazy init)
        let path = app.current_model_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        crate::ai::types::CharacterCard {
            file_path: path,
            ..Default::default()
        }
    });

    Window::new("角色卡编辑")
        .default_width(400.0)
        .open(&mut app.character_card_editor_open)
        .show(ctx, |ui| {
            egui::Grid::new("char_card_grid")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label("名称");
                    ui.text_edit_singleline(&mut card.name);
                    ui.end_row();

                    ui.label("描述");
                    ui.add(egui::TextEdit::multiline(&mut card.description)
                        .desired_rows(3));
                    ui.end_row();

                    ui.label("性格");
                    ui.add(egui::TextEdit::multiline(&mut card.personality)
                        .desired_rows(3));
                    ui.end_row();

                    ui.label("场景");
                    ui.add(egui::TextEdit::multiline(&mut card.scenario)
                        .desired_rows(3));
                    ui.end_row();

                    ui.label("示例对话");
                    ui.add(egui::TextEdit::multiline(&mut card.example_dialogs)
                        .desired_rows(5));
                    ui.end_row();

                    ui.label("系统提示词");
                    ui.add(egui::TextEdit::multiline(&mut card.system_prompt)
                        .desired_rows(3));
                    ui.end_row();

                    ui.label("TTS 音色");
                    if app.tts_voices_cache.is_empty() {
                        if ui.button("刷新音色列表").clicked() {
                            app.tts_refresh_requested = true;
                        }
                    } else {
                        let current = &card.tts_voice;
                        egui::ComboBox::from_id_source("card_tts_voice")
                            .selected_text(
                                if current.is_empty() {
                                    "跟随全局设置".to_string()
                                } else {
                                    app.tts_voices_cache
                                        .iter()
                                        .find(|v| v.voice_id == *current)
                                        .map(|v| format!("{} ({})", v.voice_name, v.gender))
                                        .unwrap_or_else(|| current.clone())
                                }
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut card.tts_voice, String::new(), "跟随全局设置");
                                for voice in &app.tts_voices_cache {
                                    let label = format!(
                                        "{} - {} ({}) [{}]",
                                        voice.voice_name, voice.voice_id, voice.gender, voice.language
                                    );
                                    ui.selectable_value(&mut card.tts_voice, voice.voice_id.clone(), label);
                                }
                            });
                    }
                    ui.end_row();
                });

            ui.add_space(12.0);

            if ui.button("保存").clicked() {
                if let Some(ref db) = app.db {
                    let _ = db.save_character_card(card);
                }
            }
        });
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: fails — `ai/mod.rs` doesn't declare `character_card_panel` module yet

- [ ] **Step 3: Commit**

```bash
git add live2d-viewer/src/ai/character_card_panel.rs
git commit -m "feat(ui): add character card editor panel"
```

---

### Task 5: `ai/mod.rs` — register new module

**Files:**
- Modify: `live2d-viewer/src/ai/mod.rs`

- [ ] **Step 1: Add `pub mod character_card_panel;`**

```rust
pub mod character_card_panel;
pub mod chat_panel;
pub mod client;
pub mod config;
pub mod settings_panel;
pub mod tts;
pub mod types;
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean

- [ ] **Step 3: Commit**

```bash
git add live2d-viewer/src/ai/mod.rs
git commit -m "feat(ai): register character_card_panel module"
```

---

### Task 6: `chat_panel.rs` — window title + editor button

**Files:**
- Modify: `live2d-viewer/src/ai/chat_panel.rs`

- [ ] **Step 1: Update window title to show character card name**

In `draw_chat_panel()`, change the `Window::new("AI 聊天")` title:

```rust
    let window_title = match app.current_character_card {
        Some(ref c) if !c.name.is_empty() => format!("AI 聊天 — {}", c.name),
        _ => "AI 聊天".to_string(),
    };

    Window::new(window_title)
```

Replace the hardcoded `"AI 聊天"` string with the `window_title` variable.

- [ ] **Step 2: Add a ✏ button next to the title to open the character card editor**

Inside the window's `show` closure, at the top (before `ui.label(...)`):

```rust
            ui.horizontal_top(|ui| {
                ui.label(format!("模型: {}", app.ai_config.model));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("✏").clicked() {
                        app.character_card_editor_open = true;
                    }
                });
            });
```

And remove the old `ui.label(format!("模型: {}", app.ai_config.model));` line.

- [ ] **Step 3: Call `draw_character_card_editor()` at end of `draw_chat_panel()`**

After the window's `.show()` closure, add:

```rust
    // Character card editor (separate window, shown/hidden via button)
    crate::ai::character_card_panel::draw_character_card_editor(ctx, app);
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean

- [ ] **Step 5: Commit**

```bash
git add live2d-viewer/src/ai/chat_panel.rs
git commit -m "feat(ui): chat panel shows character card name and editor button"
```

---

### Task 7: `settings_panel.rs` — button to open character card editor

**Files:**
- Modify: `live2d-viewer/src/ai/settings_panel.rs`

- [ ] **Step 1: Add "角色卡" button to the AI settings panel**

Add before the save button section (around line 117), inside the window closure:

```rust
            ui.add_space(12.0);
            ui.separator();
            ui.heading("角色卡");

            if let Some(ref card) = app.current_character_card {
                if !card.name.is_empty() {
                    ui.label(format!("当前: {}", card.name));
                }
            } else {
                ui.colored_label(egui::Color32::GRAY, "无角色卡");
            }

            if ui.button("编辑角色卡").clicked() {
                app.character_card_editor_open = true;
            }
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean

- [ ] **Step 3: Run full check**

```bash
cargo check --release -p live2d-viewer 2>&1
cargo fmt -p live2d-viewer
cargo clippy --release -p live2d-viewer 2>&1 | grep -E "^(warning|error)" | grep -v "generated by" | grep live2d-viewer
```

Expected all clean

- [ ] **Step 4: Commit**

```bash
git add live2d-viewer/src/ai/settings_panel.rs
git commit -m "feat(ui): settings panel has character card button"
```

---

### Final Verification

- [ ] **Full project check**

```bash
cargo check --release -p live2d-viewer 2>&1
cargo fmt -p live2d-viewer
cargo clippy --release -p live2d-viewer 2>&1 | grep -E "^warning|^error"
```

Expected: zero warnings/errors from our changes.

- [ ] **Push to remote**

```bash
git push origin main
```
