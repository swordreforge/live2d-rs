# Per-Model Character Cards — Design Spec

## Motivation

Each Live2D model should be able to have its own distinct personality, conversation style, and TTS voice. Currently a single global `system_prompt` is shared across all models. Character cards provide independent per-model identities following the spirit of community standards (TavernAI / RisingStar).

## Data Model

### SQLite table

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

`ON DELETE CASCADE` — when a model is removed from `model_history`, its character card is cleaned up automatically.

### Rust struct (`ai/types.rs`)

```rust
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

## AppState changes (`app.rs`)

New field:

```rust
pub current_character_card: Option<CharacterCard>,
pub character_card_editor_open: bool,
```

New method:

```rust
pub fn load_character_card(&mut self)
```

Called at the end of `complete_v3_switch()` (and the V2 switch equivalent). Looks up `character_cards` by the current model's `file_path`. If a card exists, sets `current_character_card`; otherwise `None`.

## Prompt construction

In `send_ai_message()` (around line 841 in current `app.rs`):

When `current_character_card` is `Some`:

1. Skip `self.ai_config.system_prompt` entirely
2. Build a synthetic system message from card fields:

```
{personality}

{scenario}

{description}

以下是 {name} 的对话示例：
{example_dialogs}

{system_prompt}
```

Each section is omitted when the corresponding field is empty. The whole block is omitted when all fields are empty (falling back to the global `system_prompt`).

## TTS voice override

In `poll_ai_result()` / the TTS spawn block (around line 917):

When `current_character_card` is `Some` and `card.tts_voice` is non-empty, use `card.tts_voice` instead of `ai_config.tts_voice`.

## DB methods (`db.rs`)

```rust
pub fn get_character_card(&self, file_path: &str) -> Result<Option<CharacterCard>>
pub fn save_character_card(&self, card: &CharacterCard) -> Result<()>
pub fn delete_character_card(&self, file_path: &str) -> Result<()>
```

All follow the existing `libsql` + `rt().block_on()` pattern used by other DB methods.

The migration adds the table if not exists (already handled by the `CREATE TABLE IF NOT EXISTS` in a new migration block inside `AppDb::open()`).

## UI — character card editor

New file: `ai/character_card_panel.rs`

A dedicated egui window (`Window::new("角色卡编辑")`) opened by `character_card_editor_open`. Fields:

| Field | Widget | Notes |
|---|---|---|
| name | `TextEdit::singleline` | Displayed in chat panel title |
| description | `TextEdit::multiline` | Background story |
| personality | `TextEdit::multiline` | Personality traits |
| scenario | `TextEdit::multiline` | Current scenario/situation |
| example_dialogs | `TextEdit::multiline` | Example conversations |
| system_prompt | `TextEdit::multiline` | Additional instructions |
| tts_voice | ComboBox | Voice selector (reuses `tts_voices_cache` from AppState; refreshes if empty) |

A "保存" button persists to DB via `db.save_character_card()`.

## Chat panel changes (`chat_panel.rs`)

The window title changes from "AI 聊天" to:

```
AI 聊天 — {card.name}
```

When there's no current character card, the title stays as "AI 聊天".

A small ✏ button next to the title opens the character card editor.

## Integration with settings panel (`settings_panel.rs`)

Add a "角色卡" button that sets `character_card_editor_open = true`.

## Implementation order

1. `db.rs` — migration + 3 new methods
2. `ai/types.rs` — `CharacterCard` struct
3. `app.rs` — `current_character_card` field + `load_character_card()` + updated `send_ai_message()` + TTS voice override
4. `ai/character_card_panel.rs` — editor UI
5. `ai/mod.rs` — `pub mod character_card_panel;`
6. `ai/chat_panel.rs` — window title update
7. `ai/settings_panel.rs` — button to open editor

## Files changed (estimated)

| File | Type |
|---|---|
| `live2d-viewer/src/db.rs` | Modify |
| `live2d-viewer/src/ai/types.rs` | Modify |
| `live2d-viewer/src/ai/mod.rs` | Modify |
| `live2d-viewer/src/app.rs` | Modify |
| `live2d-viewer/src/ai/chat_panel.rs` | Modify |
| `live2d-viewer/src/ai/settings_panel.rs` | Modify |
| `live2d-viewer/src/ai/character_card_panel.rs` | **New** |

## Non-goals

- Import/export community character card formats (PNG-embedded JSON). Can be added later as a separate feature.
- Multiple character cards per model. One card per model — simpler and matches the "character card = model identity" concept.
- Chat history per character. Currently one global conversation; character switch clears the chat or could be addressed separately.
