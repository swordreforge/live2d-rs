---
slug: model-search-tab
status: awaiting-approval
intent: clear
pending-action: write .omo/plans/model-search-tab.md
approach: Word-vector embeddings via character n-gram hashing trick (128d, FNV-1a, log-freq + L2 norm) + cosine similarity sorting
---

# Draft: model-search-tab

## Components (topology ledger)
id | outcome | status | evidence path
--- | --- | --- | ---
db embedding | embed_text() + search_models() + cosine sim + embeddings table | drafting | .omo/plans/model-search-tab.md
gui search | Search window with input + results + click-to-switch | drafting | .omo/plans/model-search-tab.md

## Open assumptions (announced defaults)
assumption | adopted default | rationale | reversible?
--- | --- | --- | ---
Embedding dimension | 128 | Sufficient for short model names; fast | Yes — change const
Hash function | FNV-1a (stable, deterministic) | Zero deps, stable across Rust versions | Yes — swap to any hash
Scaling | log-frequency (ln(1+x)) + L2 norm | Works for short text without IDF (no corpus stats needed) | Yes — swap to IDF later
CJK support | Character-level n-grams (1-3 chars) | N-gram hashing naturally handles CJK without tokenization | Yes — swap to subword/huggingface later
Embedding storage | BLOB in model_embeddings table, cached | Avoids recomputation on every search | Yes — remove cache anytime
Search scope | All model_history entries (not just loaded models) | DB has every model ever added; search should cover all | Yes — filter to loaded only

## Findings (cited - path:lines)
- gui.rs:59-308 — `draw_normal_ui()` renders "Model List" + "Parameters" as separate egui Windows
- gui.rs:103 — model entries use `ui.selectable_label(selected, &label)` for click-to-switch
- db.rs:115-121 — `model_history()` queries all records ordered by last_opened DESC
- app.rs:399-424 — `begin_switch(idx)` — async V3 / sync V2, idx indexes into `model_list`
- app.rs:137 — `model_list: Vec<ModelEntry>` with `.dir: PathBuf` matching `db.file_path`
- app.rs:237 — `db: Option<db::AppDb>` in AppState
- db.rs:40-62 — `open()` creates tables in execute_batch

## Decisions (with rationale)
1. **Hashing-trick word vectors** — not neural embeddings. Produces proper f32 vectors, cosine similarity, zero deps. FNV-1a hash is stable across Rust versions.
2. **On-the-fly embedding in search_models** — iterate all models, compute embedding + cosine sim, sort. For <100 models this is <1ms.
3. **Optional DB cache** — `model_embeddings` table stores pre-computed vectors. If available, skip recomputation.
4. **No TF-IDF** — log-frequency only. Avoiding corpus-wide IDF stats (would require recomputing when models added/removed). For short model names, log-freq works well.
5. **Separate "搜索" Window** — matches existing codebase pattern (Model List + Parameters as separate windows).

## Scope IN
- `embed_text()` — 128d, char n-gram hashing trick, log-freq, L2 norm
- `cosine_sim(a, b)` — dot product
- `search_models(query, limit)` — for each model, compute/load embedding → cosine sim → sort DESC → take top-k
- `SearchResult` struct
- `model_embeddings` table + `set/get_model_embedding()` helpers
- New "搜索" egui Window: search box → results (name + %) → click switches model
- `search_query: String` and `search_results: Vec<SearchResult>` in AppState

## Scope OUT (Must NOT have)
- No neural embedding models / ONNX / external API
- No new crate dependencies
- No FTS5 / SQLite FTS index
- No pagination
- No tab refactor
- No async

## Open questions
None.

## Approval gate
status: awaiting-approval
