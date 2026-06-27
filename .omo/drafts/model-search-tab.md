---
slug: model-search-tab
status: complete
intent: clear
approach: Word-vector embeddings via character n-gram hashing trick (128d, FNV-1a, log-freq + L2 norm) + cosine similarity sorting. AlwaysOnTop search via GL-native bitmap font overlay + sctk keyboard input.
---

## Components (topology ledger)
id | outcome | status | evidence path
--- | --- | --- | ---
db embedding | embed_text() + search_models() + cosine sim + embeddings table | complete | live2d-viewer/src/db.rs
gui search | "搜索" Window + pet toolbar popup | complete | live2d-viewer/src/gui.rs
AlwaysOnTop search | GL-native panel + bitmap font + keyboard input | complete | live2d-viewer/src/wayland_pet.rs
text renderer | 5×7 bitmap font (91 glyphs) + GL alpha texture + shader | complete | live2d-viewer/src/text_renderer.rs
IME | winit set_ime_allowed(true) for egui input | complete | live2d-viewer/src/main.rs

## Commits
- `fbd246c` feat(search): DB embedding + search tab + egui search window
- `aab5eb3` fix(ime): window.set_ime_allowed(true)
- `9222cbc` feat(pet-toolbar): search button + popup in windowed pet
- `dd71ee1` feat(pet-toolbar): GL-native search panel in AlwaysOnTop
- `d0a1f0d` fix(pet-search): separator rebind toolbar program/vao
- `a1d6b2f` fix(pet-search): GL state cleanup (texture/program unbind, depth/cull)
- `20f9baf` fix(font): glyph_scale parameter for readable 5x7 font

## Key Architecture
- AlwaysOnTop pet thread: sctk layer-shell surface, GL overlay, no egui
- Search panel: raw GL rects + TextRenderer bitmap font + wl_keyboard evdev key→char
- Keyboard: set_keyboard_interactivity(Exclusive) when search open, None when closed
- Escape closes search + restores keyboard interactivity
- Click result → respawn_process(model_dir) switches model
- Main thread: receives PetEvent::ToolbarAction(Search) → db.model_history() → PetCommand::ModelList
