# model-search-tab - Work Plan

## TL;DR (For humans)

**What you'll get:** A new "搜索" window in the Live2D Viewer. Type any text — model name, partial name, CJK characters — and it computes real word-vector embeddings (dense f32 vectors via character n-gram hashing trick) for both the query and every model name, then ranks results by cosine similarity. Click any result to switch to that model. Embeddings are cached in a `model_embeddings` DB table for future neural model upgrades.

**Why this approach:** "词向量" (word vectors) with cosine similarity is the exact semantic ranking the user asked for. The hashing-trick embedding needs zero external deps (no ONNX, no Python), works natively with CJK characters, and produces proper f32 dense vectors. When a real neural embedding model is added later, it's a one-function swap.

**What it will NOT do:** No neural transformers (no ONNX Runtime, no candle, no external API). No tab refactor. No FTS5. No pagination.

**Effort:** Short
**Risk:** Low — 2 files changed (db.rs, gui.rs), 2 fields added to AppState
**Decisions to sanity-check:** Character n-gram hashing trick as embedding (stable FNV-1a hash, 128d, log-freq + L2 norm); embeddings cached in DB but recomputed fresh if stale.

---

> TL;DR (machine): Short / Low / +2 files (db.rs + gui.rs) + 2 AppState fields — real word-vector embedding via hashing-trick, cosine similarity sort, cached in model_embeddings table

## Scope
### Must have
- `AppDb::search_models(query, limit) -> Vec<SearchResult>` — loads/embeds all model names, computes cosine similarity, returns top-k sorted
- `SearchResult` struct with `file_path, name, model_version, score: f64`
- Word-vector embedding via character n-gram hashing trick (stable FNV-1a hash, 128d, unigram+bigram+trigram, log-frequency weighted, L2 normalized)
- `model_embeddings` table with `(file_path PK, embedding BLOB, updated_at)` — cache for computed vectors
- New "搜索" egui Window: text input → results list (name + similarity %) → click switches model
- Click maps `result.file_path` → `model_list[].dir` → `app.begin_switch(idx)`
- Embeddings computed when model is added (via `add_or_update_model`) or on-demand at search time

### Must NOT have (guardrails, anti-slop, scope boundaries)
- No tab refactor — existing windows stay as-is
- No neural embedding models / ONNX / external API calls
- No new crate dependencies — hashing is manual FNV-1a, vectors are std-only
- No FTS5 / SQLite FTS index
- No pagination — limit=20 hardcoded
- No async — block_on on main thread for DB ops; embedding computation is pure CPU <1ms

## Verification strategy
- Test decision: tests-after (manual: cargo check + cargo clippy)
- Evidence: .omo/evidence/task-*.txt (build output)

## Execution strategy
### Dependency matrix
| Todo | Depends on | Blocks | Can parallelize with |
| --- | --- | --- | --- |
| 1. db.rs: embed helper + search method + schema | — | 2 | — |
| 2. gui.rs + app.rs: search UI + AppState fields | 1 | — | — |

## Todos
- [ ] 1. `db.rs`: Add `embed()` helper, `search_models()` method, `model_embeddings` table
  What to do:
  - Add private helper in db.rs (not a method, just a module-level function):
    ```rust
    /// Embed text into a 128-dimensional f32 vector via character n-gram hashing trick.
    /// Uses stable FNV-1a hash, log-frequency weighting, L2 normalization.
    /// Zero external dependencies — pure std.
    const EMBED_DIM: usize = 128;
    fn embed_text(text: &str) -> Vec<f32> {
        let mut vec = vec![0.0f32; EMBED_DIM];
        let chars: Vec<char> = text.chars().collect();
        for span_len in 1..=3 {
            for window in chars.windows(span_len) {
                let mut hash: u64 = span_len as u64;
                for &c in window {
                    hash = hash.wrapping_mul(0x100000001b3) ^ (c as u64);
                }
                let idx = (hash as usize) % EMBED_DIM;
                vec[idx] += 1.0;
            }
        }
        // Log-frequency scaling: ln(1+x)
        for v in &mut vec { *v = (*v).ln_1p(); }
        // L2 normalize
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 { for v in &mut vec { *v /= norm; } }
        vec
    }
    fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }
    ```
  - Add `pub struct SearchResult { pub file_path: String, pub name: String, pub model_version: String, pub score: f64 }`
  - Add `search_models(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>`:
    1. If query is empty → return empty
    2. Query embedding: `let qv = embed_text(query);`
    3. Fetch all models: `SELECT file_path, name, model_version FROM model_history`
    4. For each model name: compute `embed_text(name)`, cosine similarity with qv
    5. Sort DESC by score, take top `limit`
    6. Return Vec<SearchResult>
  - Add `model_embeddings` table to `open()`:
    ```sql
    CREATE TABLE IF NOT EXISTS model_embeddings (
        file_path TEXT PRIMARY KEY REFERENCES model_history(file_path),
        embedding BLOB NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    ```
  - Add `set_model_embedding(&self, file_path: &str, embedding: &[f32]) -> Result<()>` to store/update cached embedding
  - Add `get_model_embedding(&self, file_path: &str) -> Result<Option<Vec<f32>>>` to retrieve cached embedding
  - In `search_models`: if cached embedding exists for a model, use it; otherwise compute on-the-fly and optionally cache
  - Must NOT: add new crate deps, add async, modify existing AppDb API signatures
  - Must NOT: change behavior of any existing method
  References: db.rs:40-62 (open() schema creation), db.rs:115-121 (model_history query pattern), db.rs:4-16 (rt() helper)
  Acceptance criteria: `cargo check --release` passes, `cargo clippy` zero new warnings
  QA: happy — cargo check + clippy pass; edge — empty query returns empty vec
  Evidence: .omo/evidence/task-1-model-search-tab.txt
  Commit: Y | `feat(db): add word-vector embedding search (hashing-trick + cosine sim) + embeddings table`

- [ ] 2. `app.rs` + `gui.rs`: Add search state to AppState and render search window
  What to do:
  - In `app.rs` `AppState`, add two fields:
    ```rust
    pub search_query: String,
    pub search_results: Vec<db::SearchResult>,
    ```
  - Initialize in `AppState::new()`:
    ```rust
    search_query: String::new(),
    search_results: Vec::new(),
    ```
  - In `gui.rs` `draw_normal_ui()`, add third window after the Parameters window:
    ```rust
    if let Some(ref db) = app.db {
        Window::new("搜索").default_width(280.0).show(ctx, |ui| {
            let prev = app.search_query.clone();
            ui.add(egui::TextEdit::singleline(&mut app.search_query)
                .hint_text("输入模型名称搜索...")
                .desired_width(250.0));
            // Trigger search when query changes
            if app.search_query != prev || (app.search_results.is_empty() && !app.search_query.is_empty()) {
                if let Ok(results) = db.search_models(&app.search_query, 20) {
                    app.search_results = results;
                }
            }
            ui.separator();
            if app.search_query.is_empty() {
                ui.label("输入关键词开始搜索");
            } else if app.search_results.is_empty() {
                ui.label("无匹配结果");
            } else {
                for result in &app.search_results {
                    let pct = (result.score * 100.0).min(99.9);
                    let label = format!("{} (相似度: {:.0}%)", result.name, pct);
                    let resp = ui.selectable_label(false, &label);
                    if resp.clicked() {
                        // Find model in model_list by matching file_path
                        if let Some(idx) = app.model_list.iter().position(|e| {
                            e.dir.to_string_lossy() == result.file_path
                        }) {
                            let _ = app.begin_switch(idx);
                        }
                    }
                    resp.on_hover_text(&result.file_path);
                }
            }
        });
    }
    ```
  - Must NOT: modify existing Model List or Parameters windows
  - Must NOT: add new crate deps
  References: gui.rs:59-308 (draw_normal_ui), gui.rs:65 (Window::new), gui.rs:103 (selectable_label + click), app.rs:237 (db field), app.rs:258-336 (AppState::new)
  Acceptance criteria: `cargo check --release` passes, `cargo clippy` zero new warnings
  QA: happy — search input shows, typing triggers results, click switches model; edge — empty query shows hint, no matches shows "无匹配结果"
  Evidence: .omo/evidence/task-2-model-search-tab.txt
  Commit: Y | `feat(gui): add model search window with word-vector similarity ranking`

## Final verification wave
- [ ] F1. Plan compliance audit
- [ ] F2. Code quality review
- [ ] F3. Real manual QA
- [ ] F4. Scope fidelity

## Commit strategy
Two commits:
1. `feat(db): add word-vector embedding search (hashing-trick + cosine sim) + embeddings table`
2. `feat(gui): add model search window with word-vector similarity ranking`

## Success criteria
- `cargo check --release` passes cleanly
- `cargo clippy` zero new warnings
- New "搜索" window visible in normal UI mode
- Typing a query shows matching models sorted by cosine similarity
- Similarity percentage displayed
- Clicking a result switches to that model
- Empty query shows hint text
- No matches shows "无匹配结果"
