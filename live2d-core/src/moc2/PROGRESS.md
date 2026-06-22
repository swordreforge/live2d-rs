# MOC2 Module — Implementation Progress

> Tracking the reimplementation of the Cubism 2.1 .moc2 model parsing,
> interpolation, and transform pipeline in pure Rust.
>
> Reference: Python `live2d-v2-main` (the canonical implementation).
> Format version: v10 (partial v11 support: clip IDs on Mesh).

---

## Phase 1 — Parse + Resolve ✅ **DONE**

### 1a. MOC Binary Format Parser (`moc-parser/`)
- [x] `BinaryReader` (VLQ, i32, f32, strings, bits, version tracking)
- [x] `Registry` (Blob enum: Null, String, ObjArray, I32Array, F32Array, Opaque)
- [x] `Blob::UnresolvedRef` — **no longer pushed**; `ObjectRef` (tag 33) returns target
      index directly to keep Registry indices aligned with Python's flat objects list
- [x] `schema.rs` — format-level tag dispatch (tags 0–47):
  - `0` → Null, `1` → String, `15` → ObjArray
  - `16|25` → I32Array, `26|27` → F32Array
  - `33` → ObjectRef (4‑byte BE int32, NOT VLQ)
  - **all others** → `known_type_fn` (Python fallthrough)
- [x] `parse_moc()` — entry point, calls `read_typed_object` recursively

### 1b. Domain-Type Readers (`live2d-core/src/moc2/reader.rs`)
- [x] Tag dispatch: `read_known_type` handles tags 48–142
- [x] Per-type `read_*` functions:
  - `read_id` (50, 51, 60, 134)
  - `read_warp_deformer` (65) — col, row, pivot grid, opacity
  - `read_pivot_manager` (66) — array of ParamPivots
  - `read_param_pivots` (67) — pivotCount is raw i32, NOT VLQ
  - `read_rotation_deformer` (68) — affine list + opacity
  - `read_affine_ent` (69) — origin, scale, rotation, reflect bits
  - `read_mesh` (70) — vertex/index/uv arrays, clip ID, color, culling
  - `read_param_def_float` (131) — min/max/default
  - `read_parts_data` (133) — locked, visible, deformer+drawable lists
  - `read_model_impl` (136) — param set, parts list, canvas size
  - `read_param_def_set` (137) — param array
  - `read_avatar` (142) — id + drawable/deformer lists

### 1c. Resolution (`resolve.rs` + `resolve_util.rs`)
- [x] Two-pass resolution:
  1. Scan Opaque entries → build registry-index → domain-index maps
  2. Walk entries → build final domain structs resolving cross-references
- [x] Forward-reference safe (PartsData before Mesh handled correctly)
- [x] All domain types resolved:
  - `ParamDef` (id, min/max/default) — tag 131
  - `PartsData` (id, locked, visible, deformer_indices, drawable_indices) — tag 133
  - `Deformer` (Warp / Rotation) with pivot opacity — tags 65, 68
  - `PivotManager` (param pivot indices) — tag 66
  - `ParamPivot` (param_id, count, values) — tag 67
  - `AffineEnt` (origin, scale, rotation, reflect) — tag 69
  - `Drawable` (id, target, pivot manager, mesh data, clip) — tag 70
  - `ModelImpl` (canvas width/height extracted) — tag 136
- [x] Render order pre-computed (sort by `average_draw_order`)
- [x] `resolve_f32_array`, `resolve_i32_array`, `resolve_u16_array`,
      `resolve_obj_array`, `resolve_id`, `resolve_ref`

### Integration Test
- [x] `tests/parse_moc2.rs` — parses `live2d-v2-main/test-data/model.moc`
- [x] Verifies: canvas size, param count, parts count, prints all structures
- [x] **42 tests pass, zero compiler warnings**

### Phase 1 Known Gaps
- Current test file has **0 Mesh (tag 70) entries** — parameter/deformer-only model
- Tag 109 present in the test file (Python also doesn't handle it — it's likely
  Cubism 2.2+ format extension)
- Need a model file from `CubismSdkForNative/Resources/` to test Mesh resolution

---

## Phase 2 — Pivot Interpolation ✅ **DONE**

### 2a. Core Module (`pivot.rs`) —  **fully implemented**
- [x] Constants: `GOSA` (0.0001), `PIVOT_TABLE_SIZE` (65), `MAX_INTERPOLATION` (5), `PARAM_INDEX_NOT_INIT` (-2)
- [x] `ParamPivotState` — per-ParamPivot mutable runtime (param_index cache, tmp_pivot_index, tmp_t)
- [x] `PivotContext` — parameter provider context (values, update flags, IDs, version, setup flag)
  - Methods: `resolve_param_index`, `get_param_value`, `is_param_updated`

### 2b. PivotManager Logic — **fully implemented**
- [x] `check_param_updated` — fast-path gate: reverse-iterates owned ParamPivots, returns true if
      any relevant param changed (caches param index via `init_version`)
- [x] `calc_pivot_values` — segment search for each ParamPivot:
  - count=1: EPS check against single pivot value
  - count≥2: range check → forward scan with GOSA epsilon
  - Returns `dim_count` (# of params with non-zero t)
  - Sets `tmp_pivot_index` and `tmp_t` per state
- [x] `calc_pivot_indices` — builds multi-dimensional corner lookup table:
  - `tmp_indices[0..2^dim_count]`: binary-interleaved index pattern
  - `tmp_t[0..dim_count]`: interpolation factors per dimension
  - Sentinels at end of each array (65535, -1.0)
- [x] `weight_for_vertex` — computes multi-linear blend weight for one hypercube corner

### 2c. UtInterpolate Equivalents — **fully implemented**
- [x] `interpolate_float` — weighted-sum multi-linear interpolation of `&[f32]`
- [x] `interpolate_int` — same but i32, rounding with `+0.5`
- [x] `interpolate_points` — element-wise multi-linear interpolation of point arrays
- [x] All three use generic multi-linear fallback (no unrolled special cases — Rust's optimizer
      handles the small-dim cases just as well)

### Tests — **25 unit tests, all passing**
- 7× `calc_pivot_values`: single pivot on/off, two-pivot in-range/at-exact/below/above, multi-param dim_count
- 1× `calc_pivot_values` version cache: verifies param_index caching across frames
- 3× `calc_pivot_indices`: no-interp, one-interp, both-interp
- 3× `interpolate_float`: 0D (copy), 1D (lerp), 2D (bilinear), 2D outside flag
- 2× `interpolate_int`: 1D, 3D generic fallback (verified 4400 = weighted sum)
- 3× `interpolate_points`: 0D (copy), 1D (lerp elementwise), 2D (bilinear 4.4, 5.4)
- 4× `check_param_updated`: setup, no-change, changed, unrelated-params

---

## Phase 3 — Deformer Interpolation 🔲 **NOT STARTED** (stubs only)

### 3a. WarpDeformer
**Reference: `live2d/core/deformer/warp_deformer.py`**

- `setupInterpolate(mdc, warp_ctx)` → `WarpContext`
  - Check param update; if unchanged, return early
  - `interpolatePoints` on the pivot grid → store in `context.interpolatedPoints`
  - `interpolateOpacity` → store in `context.interpolatedOpacity`

- `setupTransform(mdc, warp_ctx)` → `WarpContext`
  - Chain through parent deformer if `needTransform()`
  - Calls parent's `transformPoints` on interpolated grid → `transformedPoints`
  - Propagates total opacity through hierarchy

- `transformPoints(mdc, warp_ctx, src, dst, numPoints, offset, step)`
  - If `transformedPoints` available, use those; else use `interpolatedPoints`
  - Delegate to `transformPoints_sdk2` static method

- **`transformPoints_sdk2`** (the core warp algorithm)
  - For each input vertex (UV coords in [0,row]×[0,col] space):
    - If **in bounds**: bilinear interpolation within the grid cell
    - If **out of bounds**: extrapolate using nearest grid cell + corner averages
    - Edge cases for all 8 directions (top, bottom, left, right, 4 corners)
    - `(row+1)×(col+1)` control points, 2 floats each (x, y)

### 3b. RotationDeformer
**Reference: `live2d/core/deformer/roation_deformer.py`**

- `setupInterpolate(mdc, rot_ctx)` → `RotationContext`
  - `calcPivotValues` → `calcPivotIndices`
  - For pivot_count_bit $`n`$ (0..4):
    - `n=0`: copy single affine
    - `n=1`: lerp between 2 affines (5 fields each)
    - `n=2`: bilinear blend of 4 affines
    - `n=3`: trilinear blend of 8 affines
    - `n=4`: quad-linear blend of 16 affines
    - `n≥5`: generic binary-weight sum of $`2^n`$ affines
  - Reflect flags always taken from first affine

- `setupTransform(mdc, rot_ctx)` → `RotationContext`
  - Chain to parent deformer
  - Uses iterative direction search (`getDirectionOnDst`) to measure parent's
    non-uniform rotation (rotational component accumulation)
  - Computes `transformedAffine`:
    - origin: transformed via parent
    - scale/rotation: composite of interpolated + parent contribution
  - `setTotalScale`, `setTotalOpacity` with parent multiplication

- `transformPoints(mdc, rot_ctx, src, dst, numPoints, offset, step)`
  - Standard 2D affine: $`x' = s·cosθ·x - s·sinθ·y + ox`$ (with reflect)
  - Matrix: `[ a, b ]` where `a = s·cosθ·rx`, `b = -s·sinθ·ry`
            `[ c, d ]`       `c = s·sinθ·rx`, `d = s·cosθ·ry`

- **`getDirectionOnDst(mdc, target_deformer, target_ctx, srcOrigin, srcDir, retDir)`**
  - Iterative search for direction vector through parent deformer chain
  - Starts at step = 1, shrinks by 0.1× per iteration until non-zero found
  - Used to measure rotation introduced by parent transform

### 3c. DeformerContext types
- `WarpContext` — `interpolatedPoints`, `transformedPoints`, opacity, available flag
- `RotationContext` — `interpolatedAffine`, `transformedAffine`, opacity, scale, available

### Phase 3 Files to Create/Modify
- `live2d-core/src/moc2/deformer.rs` — **currently 2-line stub** → full WarpDeformer + RotationDeformer impl

---

## Phase 4 — Runtime Model 🔲 **NOT STARTED** (stubs only)

### 4a. Moc2Model (`runtime.rs`)
**Reference: `live2d/core/model_context.py`**

Wraps `Moc2Data` + mutable runtime state:

- **Parameter storage**: `param_values: [f32]`, `param_min: [f32]`, `param_max: [f32]`
- **Parameter update tracking**: `updated_param_flags: [bool]`, version counter
- **Parameter operations**: `get_param_index(id)`, `get_param_float(idx)`, `set_param_float(idx, val)`
- **`get_param(id)`** → resolve string id to index at runtime

- **Deformer contexts**: `Vec<WarpContext>` + `Vec<RotationContext>` (allocated once)
- **Deformer operations**: `get_deformer_index(target_id)`, `get_deformer(idx)`, `get_deformer_context(idx)`

- **Parts contexts**: visibility/opacity tracking per `PartsData`

- **Draw context**: per-drawable transformed vertex/index buffers

- **`update()` entry point**:
  1. Check which params changed
  2. For each deformer in tree order:
     - `setupInterpolate` *→* `setupTransform`
  3. For each drawable:
     - Resolve target deformer chain → transform mesh vertices
     - Apply parts opacity

### Phase 4 Files to Create/Modify
- `live2d-core/src/moc2/runtime.rs` — currently 2-line stub → full runtime model
- May need: `deformer_context.rs` for WarpContext/RotationContext types

---

## Phase 5 — Viewer Integration 🔲 **NOT STARTED**

### 5a. 
`live2d-viewer/src/model_loader.rs`
- Detect MOC2 files (magic number) vs MOC3
- Load via `parse_moc2()` instead of Cubism Core
- Create `gl` render bridges for Drawable mesh data

### 5b. Rendering pipeline
- Vertex buffer upload: transformed positions + UVs
- Index buffer for triangle strips
- Color blending: multiply/screen/normal per drawable
- Clipping masks (for drawables with clip_id)
- Part visibility / opacity sort + alpha blending

---

## Test Data Needed

| Model Type | Tags | Source |
|---|---|---|
| ✅  Parameter-only | 1× ModelImpl, 27 ParamDef, 15 PartsData, 1 WarpDeformer, 1 PivotManager, 2 ParamPivots | `live2d-v2-main/test-data/model.moc` |
| ❌  Mesh + Drawables | 70 (Mesh), index/uv arrays, pivot draw orders | Needs `CubismSdkForNative/Resources/` model |
| ❌  RotationDeformer | 68 (RotationDeformer), 69 (AffineEnt), multi-pivot | Needs model with bone-like rotation deformers |
| ❌  Clip masks | 70 (Mesh with clip_id non-null) | Format version ≥ 11 model |
| ❌  Multi-deformer chain | Parent-child deformer transforms | Full character model from SDK Resources |

---

## Implementation Order (Recommended)

```
Phase 2a: PivotManager.calcPivotValues + calcPivotIndices
Phase 2b: ParamPivot runtime state (getParamIndex/setParamIndex, tmp vars)
Phase 2c: UtInterpolate (interpolateFloat, interpolateInt, interpolatePoints)
─────────────────────────────────────────────────────────────
Phase 3a: WarpContext + WarpDeformer (setupInterpolate, setupTransform, transformPoints)
Phase 3b: RotationContext + RotationDeformer (setupInterpolate, setupTransform, transformPoints)
Phase 3c: Deformer base (interpolateOpacity, needTransform)
─────────────────────────────────────────────────────────────
Phase 4:  Moc2Model runtime (parameter storage, deformer chain, update entry point)
Phase 5:  Viewer integration (MOC2 detection, OpenGL render bridge)
```
