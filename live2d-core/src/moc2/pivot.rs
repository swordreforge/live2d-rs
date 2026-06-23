//! Pivot interpolation engine (Phase 2).
//!
//! This module implements the multi-parameter pivot interpolation system
//! from Cubism 2.1.  A PivotManager owns a set of ParamPivots — each
//! defines the "pivot points" (key values) for one model parameter.
//! Together they form a multi-dimensional interpolation space.
//!
//! The pipeline for any interpolated value is:
//!
//! 1. [`check_param_updated`] — fast path: skip if no relevant param changed
//! 2. [`calc_pivot_values`] — for each ParamPivot, find which segment the
//!    current parameter value falls in, compute interpolation factor `t`
//! 3. [`calc_pivot_indices`] — build the multi-dimensional corner lookup
//!    table (`tmp_indices`) and interpolation factors (`tmp_t`)
//! 4. Multi-linear blend using the computed indices
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]
//!
//! Reference: `live2d/core/param/pivot_manager.py`,
//!            `live2d/core/util/ut_interpolate.py`

use super::types::{Id, ParamPivot};

// ── constants ──────────────────────────────────────────────────────

/// Epsilon for floating-point equality checks in pivot segment search.
pub(crate) const GOSA: f32 = 0.0001;

/// Maximum size of the pivot table index array (= 2⁶ + 1).
pub(crate) const PIVOT_TABLE_SIZE: usize = 65;

/// Maximum number of interpolation dimensions.
#[allow(dead_code)]
pub(crate) const MAX_INTERPOLATION: usize = 5;

/// Sentinel value for an uninitialized param index.
pub(crate) const PARAM_INDEX_NOT_INIT: i32 = -2;

// ── runtime state ──────────────────────────────────────────────────

/// Per-`ParamPivot` mutable runtime state.
///
/// One instance per `ParamPivot`, allocated once and reused each frame.
/// Tracks the cached parameter index (lazily resolved), the per-frame
/// interpolation state (`tmp_pivot_index`, `tmp_t`), and an `init_version`
/// counter for cache invalidation.
#[derive(Debug, Clone)]
pub(crate) struct ParamPivotState {
    /// Cached index into the model's parameter array (`-2` = uninit, `-1` = not found).
    pub param_index: i32,
    /// The model `init_version` when `param_index` was last resolved.
    pub init_version: i32,
    /// Per-frame: which segment/pivot the current value falls in.
    pub tmp_pivot_index: i32,
    /// Per-frame: interpolation factor within that segment.
    pub tmp_t: f32,
}

impl Default for ParamPivotState {
    fn default() -> Self {
        Self {
            param_index: PARAM_INDEX_NOT_INIT,
            init_version: -1,
            tmp_pivot_index: 0,
            tmp_t: 0.0,
        }
    }
}

// ── model context (borrowed from Moc2Model) ────────────────────────

/// Minimal parameter-provider context that the pivot system reads from.
///
/// Constructed by [`Moc2Model`][crate::moc2::runtime::Moc2Model] each
/// frame (Phase 4).  Keeps the pivot functions pure and testable.
#[derive(Debug, Clone)]
pub(crate) struct PivotContext<'a> {
    /// Current parameter values, indexed by param position.
    pub param_values: &'a [f32],
    /// Per-param update flags (`true` = changed this frame).
    pub param_updated: &'a [bool],
    /// Parameter ID at each position (for lazy resolution).
    pub param_ids: &'a [Id],
    /// Version counter — incremented each frame; used to invalidate
    /// cached param indices from previous frames.
    pub init_version: i32,
    /// If `true`, skip the change-detection check and always update.
    pub setup_required: bool,
}

impl PivotContext<'_> {
    /// Resolve a parameter ID to its index in the values/updated arrays.
    pub fn resolve_param_index(&self, param_id: &Id) -> Option<usize> {
        self.param_ids.iter().position(|id| id == param_id)
    }

    /// Read the current value of a parameter.
    pub fn get_param_value(&self, index: usize) -> f32 {
        self.param_values.get(index).copied().unwrap_or(0.0)
    }

    /// Check whether a parameter was updated this frame.
    pub fn is_param_updated(&self, index: usize) -> bool {
        self.param_updated.get(index).copied().unwrap_or(false)
    }
}

// ── PivotManager methods ───────────────────────────────────────────

/// Check whether any parameter owned by this PivotManager has changed.
///
/// Returns `true` if setup is required or any parameter was updated.
/// This is the fast-path gate — callers can skip the full interpolation
/// when nothing changed.
///
/// Reference: `PivotManager.checkParamUpdated`
pub(crate) fn check_param_updated(
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
) -> bool {
    if ctx.setup_required {
        return true;
    }

    for &pivot_idx in param_pivot_indices.iter().rev() {
        let state = &mut pivot_states[pivot_idx];
        let pp = &param_pivots[pivot_idx];

        // Resolve param index if not cached for this version.
        if state.init_version != ctx.init_version {
            state.param_index = ctx
                .resolve_param_index(&pp.param_id)
                .map(|i| i as i32)
                .unwrap_or(-1);
            state.init_version = ctx.init_version;
        }

        let idx = state.param_index;
        if idx >= 0 && ctx.is_param_updated(idx as usize) {
            return true;
        }
    }

    false
}

/// Compute pivot-segment indices and interpolation factors for every
/// ParamPivot owned by a PivotManager.
///
/// For each ParamPivot:
/// - Resolves the parameter index (cached via `init_version`).
/// - Reads the current parameter value.
/// - Binary-searches the pivot value array to find the segment.
/// - Stores `tmp_pivot_index` and `tmp_t` in the corresponding state.
///
/// Returns the number of parameters with `tmp_t ≠ 0` (the "dimension
/// count" used by `calc_pivot_indices`).
///
/// `ret_param_out` is set to `true` if ANY parameter value fell
/// outside its defined pivot range.
///
/// Reference: `PivotManager.calcPivotValues`
pub(crate) fn calc_pivot_values(
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    ret_param_out: &mut bool,
) -> usize {
    let mut dim_count = 0usize;

    for &pivot_idx in param_pivot_indices.iter() {
        let state = &mut pivot_states[pivot_idx];
        let pp = &param_pivots[pivot_idx];

        // Resolve & cache param index.
        if state.init_version != ctx.init_version {
            state.param_index = ctx
                .resolve_param_index(&pp.param_id)
                .map(|i| i as i32)
                .unwrap_or(-1);
            state.init_version = ctx.init_version;
        }

        let param_idx = state.param_index;
        let value = if param_idx >= 0 {
            ctx.get_param_value(param_idx as usize)
        } else {
            0.0
        };

        let count = pp.pivot_count;
        let vals = &pp.pivot_values;
        let mut pivot_index = -1i32;
        let mut t = 0.0f32;

        if count >= 1 {
            if count == 1 {
                // Single pivot value — check if value is "near" it.
                let s = vals[0];
                if (s - GOSA..=s + GOSA).contains(&value) {
                    pivot_index = 0;
                    t = 0.0;
                } else {
                    pivot_index = 0;
                    t = 0.0;
                    *ret_param_out = true;
                }
            } else {
                // Two or more pivot values — binary-search the segment.
                let mut prev = vals[0];
                if value < prev - GOSA {
                    // Below the first pivot.
                    pivot_index = 0;
                    t = 0.0;
                    *ret_param_out = true;
                } else if value <= prev + GOSA {
                    // At the first pivot exactly.
                    pivot_index = 0;
                    t = 0.0;
                } else {
                    let mut found = false;
                    for i in 1..count as usize {
                        let cur = vals[i];
                        if value < cur + GOSA {
                            if value > cur - GOSA {
                                // Exactly at this pivot.
                                pivot_index = i as i32;
                                t = 0.0;
                            } else {
                                // Between prev and cur.
                                pivot_index = (i - 1) as i32;
                                t = (value - prev) / (cur - prev);
                                dim_count += 1;
                            }
                            found = true;
                            break;
                        }
                        prev = cur;
                    }
                    if !found {
                        // Above the last pivot.
                        pivot_index = count - 1;
                        t = 0.0;
                        *ret_param_out = true;
                    }
                }
            }
        }
        // count < 1: leave pivot_index = -1, t = 0 (default)

        state.tmp_pivot_index = pivot_index;
        state.tmp_t = t;
    }

    dim_count
}

/// Build the multi-dimensional corner lookup table from per-ParamPivot
/// interpolation state.
///
/// `dim_count` is the number of parameters with non-zero `t` (returned
/// by `calc_pivot_values`).
///
/// Outputs:
/// - `tmp_indices[0 .. 2^dim_count]`: corner indices into the pivot
///   value array (terminated by sentinel `65535` at `tmp_indices[2^dim_count]`)
/// - `tmp_t[0 .. dim_count]`: interpolation factors for each dimension
///   (terminated by sentinel `-1.0` at `tmp_t[dim_count]`)
///
/// Reference: `PivotManager.calcPivotIndices`
pub(crate) fn calc_pivot_indices(
    param_pivot_indices: &[usize],
    pivot_states: &[ParamPivotState],
    param_pivots: &[ParamPivot],
    dim_count: usize,
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) {
    let table_size = 1usize << dim_count;

    if table_size + 1 > tmp_indices.len() {
        // Should never happen with correct PIVOT_TABLE_SIZE.
        return;
    }

    // Zero-initialize.
    for i in 0..table_size {
        tmp_indices[i] = 0;
    }

    let mut stride: usize = 1;
    let mut block: usize = 1;
    let mut t_idx: usize = 0;

    for &pivot_idx in param_pivot_indices.iter() {
        let state = &pivot_states[pivot_idx];
        let pp = &param_pivots[pivot_idx];

        if state.tmp_t == 0.0 {
            // No interpolation — constant offset across all corners.
            let offset = (state.tmp_pivot_index as usize).wrapping_mul(stride);
            for i in 0..table_size {
                tmp_indices[i] = tmp_indices[i].wrapping_add(offset as u16);
            }
        } else {
            // Interpolation — alternating block pattern.
            let low = (state.tmp_pivot_index as usize).wrapping_mul(stride);
            let high = ((state.tmp_pivot_index as usize) + 1).wrapping_mul(stride);
            for i in 0..table_size {
                let add = if (i / block) % 2 == 0 { low } else { high };
                tmp_indices[i] = tmp_indices[i].wrapping_add(add as u16);
            }
            if t_idx < tmp_t.len() {
                tmp_t[t_idx] = state.tmp_t;
            }
            t_idx += 1;
            block *= 2;
        }

        stride = stride.wrapping_mul(pp.pivot_count as usize);
    }

    // Sentinels.
    if table_size < tmp_indices.len() {
        tmp_indices[table_size] = 65535;
    }
    if t_idx < tmp_t.len() {
        tmp_t[t_idx] = -1.0;
    }
}

// ── multi-linear interpolation helpers ─────────────────────────────

/// Compute the multi-linear blend weight for `vertex_index` (a binary
/// code) given `t` values for each dimension.
///
/// For each dimension `d`:
/// - if bit `d` of `vertex_index` is `0`: weight *= (1 - t[d])
/// - if bit `d` of `vertex_index` is `1`: weight *= t[d]
fn weight_for_vertex(vertex_index: usize, t: &[f32], dim_count: usize) -> f32 {
    let mut w = 1.0f32;
    let mut bits = vertex_index;
    for d in 0..dim_count {
        w *= if bits & 1 == 0 {
            1.0 - t[d]
        } else {
            t[d]
        };
        bits >>= 1;
    }
    w
}

// ── interpolation functions (UtInterpolate equivalents) ────────────

/// Interpolate a single `f32` value using the pivot system.
///
/// `pivot_values` is the flat array of values at each pivot configuration
/// — indexed by `tmp_indices[]` after `calc_pivot_indices`.
///
/// Returns `(interpolated_value, param_outside)`.
///
/// Reference: `UtInterpolate.interpolateFloat`
pub(crate) fn interpolate_float(
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    pivot_values: &[f32],
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) -> (f32, bool) {
    let mut outside = false;
    let dim_count = calc_pivot_values(
        param_pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        &mut outside,
    );

    if dim_count == 0 {
        return (pivot_values[0], outside);
    }

    calc_pivot_indices(
        param_pivot_indices,
        pivot_states,
        param_pivots,
        dim_count,
        tmp_indices,
        tmp_t,
    );

    let num_vertices = 1usize << dim_count;

    // Pre-compute weights.
    let mut weights = [0.0f32; 64]; // 2⁶ = max table size
    debug_assert!(num_vertices <= weights.len());
    for v in 0..num_vertices {
        weights[v] = weight_for_vertex(v, tmp_t, dim_count);
    }

    // Weighted sum.
    let mut result = 0.0f32;
    for v in 0..num_vertices {
        let idx = tmp_indices[v] as usize;
        result += weights[v] * pivot_values[idx];
    }

    (result, outside)
}

/// Interpolate a single `i32` value using the pivot system.
///
/// Returns `(interpolated_value, param_outside)`.
///
/// Reference: `UtInterpolate.interpolateInt`
pub(crate) fn interpolate_int(
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    pivot_values: &[i32],
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) -> (i32, bool) {
    let mut outside = false;
    let dim_count = calc_pivot_values(
        param_pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        &mut outside,
    );

    if dim_count == 0 {
        return (pivot_values[0], outside);
    }

    calc_pivot_indices(
        param_pivot_indices,
        pivot_states,
        param_pivots,
        dim_count,
        tmp_indices,
        tmp_t,
    );

    let num_vertices = 1usize << dim_count;

    let mut weights = [0.0f32; 64];
    debug_assert!(num_vertices <= weights.len());
    for v in 0..num_vertices {
        weights[v] = weight_for_vertex(v, tmp_t, dim_count);
    }

    let mut result = 0.0f32;
    for v in 0..num_vertices {
        let idx = tmp_indices[v] as usize;
        result += weights[v] * pivot_values[idx] as f32;
    }

    ((result + 0.5) as i32, outside)
}

/// Interpolate an array of 2-D points through the pivot system.
///
/// Each pivot configuration stores a flat array of `(x, y)` co-ordinate
/// pairs for every point.  The output `dst` is filled with the
/// interpolated `(x, y)` pairs, one per point.
///
/// # Parameters
/// - `pivot_points`: flat array indexed by
///   `corner_index * points_per_corner + coord_index`
/// - `points_per_corner`: number of floats stored per corner (typically
///   `num_points * 2`)
/// - `num_points`: how many (x, y) pairs to compute in the output
///
/// Returns `true` if any parameter was outside its pivot range.
///
/// Reference: `UtInterpolate.interpolatePoints`
#[allow(dead_code)]
pub(crate) fn interpolate_points(
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    pivot_points: &[f32],
    points_per_corner: usize,
    num_points: usize,
    dst: &mut [f32],
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) -> bool {
    let mut outside = false;
    let dim_count = calc_pivot_values(
        param_pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        &mut outside,
    );

    if dim_count == 0 {
        let src = &pivot_points[..num_points * 2];
        dst[..num_points * 2].copy_from_slice(src);
        return outside;
    }

    calc_pivot_indices(
        param_pivot_indices,
        pivot_states,
        param_pivots,
        dim_count,
        tmp_indices,
        tmp_t,
    );

    let num_vertices = 1usize << dim_count;
    let coord_count = num_points * 2;

    // Pre-compute weights.
    let mut weights = [0.0f32; 64];
    debug_assert!(num_vertices <= weights.len());
    for v in 0..num_vertices {
        weights[v] = weight_for_vertex(v, tmp_t, dim_count);
    }

    // For each coordinate, compute weighted sum across all corners.
    for ci in 0..coord_count {
        let mut sum = 0.0f32;
        for v in 0..num_vertices {
            let corner = tmp_indices[v] as usize;
            sum += weights[v] * pivot_points[corner * points_per_corner + ci];
        }
        dst[ci] = sum;
    }

    outside
}

// ── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moc2::types::ParamPivot;
    use std::sync::Arc;

    fn make_id(s: &str) -> Id {
        Arc::from(s)
    }

    fn make_pivot(id: &str, count: i32, values: Vec<f32>) -> ParamPivot {
        ParamPivot {
            param_id: make_id(id),
            pivot_count: count,
            pivot_values: values,
        }
    }

    // ── calc_pivot_values tests ─────────────────────────────────

    #[test]
    fn test_calc_pivot_values_single_pivot_at_value() {
        // Single pivot value, param value matches.
        let pivots = vec![make_pivot("ParamA", 1, vec![0.5])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 0);
        assert!(!outside);
        assert_eq!(states[0].tmp_pivot_index, 0);
        assert!((states[0].tmp_t - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_calc_pivot_values_single_pivot_far() {
        // Single pivot, value is far from the pivot point.
        let pivots = vec![make_pivot("ParamA", 1, vec![0.5])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[1.0],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 0);
        assert!(outside, "should mark outside when value != pivot");
        assert_eq!(states[0].tmp_pivot_index, 0);
    }

    #[test]
    fn test_calc_pivot_values_two_pivots_in_range() {
        // Two pivot values [0.0, 1.0], value at 0.3 → segment 0, t=0.3
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.3],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 1);
        assert!(!outside);
        assert_eq!(states[0].tmp_pivot_index, 0);
        assert!((states[0].tmp_t - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_calc_pivot_values_two_pivots_at_second() {
        // Exactly at the second pivot.
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[1.0],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 0, "t=0 at exact pivot");
        assert!(!outside);
        assert_eq!(states[0].tmp_pivot_index, 1);
    }

    #[test]
    fn test_calc_pivot_values_below_range() {
        // Value below first pivot.
        let pivots = vec![make_pivot("ParamA", 3, vec![0.0, 0.5, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[-0.1],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 0);
        assert!(outside, "below range should mark outside");
        assert_eq!(states[0].tmp_pivot_index, 0);
    }

    #[test]
    fn test_calc_pivot_values_above_range() {
        // Value above last pivot.
        let pivots = vec![make_pivot("ParamA", 3, vec![0.0, 0.5, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[2.0],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0], &pivots, &mut states, &ctx, &mut outside);

        assert_eq!(dim, 0);
        assert!(outside, "above range should mark outside");
        assert_eq!(states[0].tmp_pivot_index, 2); // last index
    }

    #[test]
    fn test_calc_pivot_values_multi_param_dim_count() {
        // Two params: one in middle of segment, one at exact pivot.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 3, vec![0.0, 0.5, 1.0]),
        ];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];
        let ctx = PivotContext {
            param_values: &[0.3, 0.5],
            param_updated: &[false, false],
            param_ids: &[make_id("ParamA"), make_id("ParamB")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside = false;

        let dim = calc_pivot_values(&[0, 1], &pivots, &mut states, &ctx, &mut outside);

        // ParamA at 0.3 → t=0.3 (counts)
        // ParamB at 0.5 → at exact pivot 1 → t=0 (doesn't count)
        assert_eq!(dim, 1);
        assert!(!outside);
        assert_eq!(states[0].tmp_pivot_index, 0);
        assert!((states[0].tmp_t - 0.3).abs() < 1e-6);
        assert_eq!(states[1].tmp_pivot_index, 1);
        assert!((states[1].tmp_t - 0.0).abs() < 1e-6);
    }

    // ── calc_pivot_indices tests ─────────────────────────────────

    #[test]
    fn test_calc_pivot_indices_one_param_no_interp() {
        // One param, no interpolation (t=0).
        let pivots = vec![make_pivot("ParamA", 3, vec![0.0, 0.5, 1.0])];
        let states = vec![ParamPivotState {
            tmp_pivot_index: 1, // at second pivot
            tmp_t: 0.0,
            ..Default::default()
        }];

        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];
        let dim_count = 0;

        calc_pivot_indices(&[0], &states, &pivots, dim_count, &mut indices, &mut t);

        // dim_count=0 → table_size=1.
        // stride=1, pivotCount=3. tmp_t=0 → offset = 1*1 = 1 added to index[0].
        // stride becomes 1*3=3 (but table_size=1, so no more entries).
        assert_eq!(indices[0], 1);
        assert_eq!(indices[1], 65535);
        assert!((t[0] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_calc_pivot_indices_one_param_with_interp() {
        // One param, with interpolation.
        let pivots = vec![make_pivot("ParamA", 3, vec![0.0, 0.5, 1.0])];
        let states = vec![ParamPivotState {
            tmp_pivot_index: 0,
            tmp_t: 0.3,
            ..Default::default()
        }];

        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];
        let dim_count = 1;

        calc_pivot_indices(&[0], &states, &pivots, dim_count, &mut indices, &mut t);

        // dim_count=1 → table_size=2. stride=1, block=1.
        // tmp_t != 0 → alternating: indices[0]=0*1=0, indices[1]=1*1=1.
        // t[0] = 0.3, t[1] = -1 sentinel.
        assert_eq!(indices[0], 0);
        assert_eq!(indices[1], 1);
        assert_eq!(indices[2], 65535);
        assert!((t[0] - 0.3).abs() < 1e-6);
        assert!((t[1] - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_calc_pivot_indices_two_params_one_interp() {
        // Two params: first interpolating, second at exact pivot.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 3, vec![0.0, 0.5, 1.0]),
        ];
        let states = vec![
            ParamPivotState {
                tmp_pivot_index: 0,
                tmp_t: 0.3,
                ..Default::default()
            },
            ParamPivotState {
                tmp_pivot_index: 1,
                tmp_t: 0.0,
                ..Default::default()
            },
        ];

        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];
        let dim_count = 1; // only first param has t != 0

        calc_pivot_indices(&[0, 1], &states, &pivots, dim_count, &mut indices, &mut t);

        // dim_count=1 → table_size=2.
        // Pass 1 (ParamA): stride=1, block=1, tmp_t=0.3 → alternating:
        //   indices[0]=0, indices[1]=1. stride → 2.
        // Pass 2 (ParamB): stride=2, tmp_t=0 → offset = 1*2 = 2:
        //   indices[0]=0+2=2, indices[1]=1+2=3. stride → 6.
        // t[0]=0.3, t[1]=-1.
        assert_eq!(indices[0], 2);
        assert_eq!(indices[1], 3);
        assert_eq!(indices[2], 65535);
    }

    #[test]
    fn test_calc_pivot_indices_two_params_both_interp() {
        // Two params, both interpolating.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 2, vec![0.0, 1.0]),
        ];
        let states = vec![
            ParamPivotState {
                tmp_pivot_index: 0,
                tmp_t: 0.3,
                ..Default::default()
            },
            ParamPivotState {
                tmp_pivot_index: 0,
                tmp_t: 0.7,
                ..Default::default()
            },
        ];

        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];
        let dim_count = 2;

        calc_pivot_indices(&[0, 1], &states, &pivots, dim_count, &mut indices, &mut t);

        // dim_count=2 → table_size=4.
        // Pass 1 (ParamA): stride=1, block=1, tmp_t=0.3 → alternating (size 1):
        //   indices[0]=0, indices[1]=1, indices[2]=0, indices[3]=1. stride → 2.
        // Pass 2 (ParamB): stride=2, block=2, tmp_t=0.7 → alternating (size 2):
        //   indices[0]=0+0=0, indices[1]=1+0=1, indices[2]=0+2=2, indices[3]=1+2=3.
        //   stride → 4.
        // t[0]=0.3, t[1]=0.7, t[2]=-1.
        assert_eq!(indices[0], 0);
        assert_eq!(indices[1], 1);
        assert_eq!(indices[2], 2);
        assert_eq!(indices[3], 3);
        assert_eq!(indices[4], 65535);
        assert!((t[0] - 0.3).abs() < 1e-6);
        assert!((t[1] - 0.7).abs() < 1e-6);
        assert!((t[2] - (-1.0)).abs() < 1e-6);
    }

    // ── interpolate_float tests ──────────────────────────────────

    #[test]
    fn test_interpolate_float_0d() {
        // dim_count=0: just copy the single corner value.
        let pivots = vec![make_pivot("ParamA", 1, vec![0.5])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5], // matches the only pivot → t=0, dim=0
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let pivot_values = vec![42.0f32]; // 1 pivot config
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (result, outside) = interpolate_float(
            &[0], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        assert!((result - 42.0).abs() < 1e-6);
        assert!(!outside);
    }

    #[test]
    fn test_interpolate_float_1d() {
        // 1-D interpolation between 2 values.
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.3],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        // pivot_values has 2 entries (one per pivot configuration)
        let pivot_values = vec![10.0f32, 20.0f32];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (result, outside) = interpolate_float(
            &[0], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        // result = 10*(1-0.3) + 20*0.3 = 7 + 6 = 13
        assert!((result - 13.0).abs() < 1e-5);
        assert!(!outside);
    }

    #[test]
    fn test_interpolate_float_2d() {
        // 2-D bilinear interpolation.
        // ParamA and ParamB with 2 pivots each = 4 pivot configs.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 2, vec![0.0, 1.0]),
        ];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];
        let ctx = PivotContext {
            param_values: &[0.3, 0.7],
            param_updated: &[false, false],
            param_ids: &[make_id("ParamA"), make_id("ParamB")],
            init_version: 0,
            setup_required: false,
        };
        // pivot_values: [V00, V01, V10, V11]
        // corner indices: 0=ParamA=0,ParamB=0; 1=ParamA=1,ParamB=0;
        //                 2=ParamA=0,ParamB=1; 3=ParamA=1,ParamB=1
        let pivot_values = vec![0.0f32, 10.0f32, 20.0f32, 30.0f32];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (result, outside) = interpolate_float(
            &[0, 1], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        // Bilinear: (1-0.7)*((1-0.3)*0 + 0.3*10) + 0.7*((1-0.3)*20 + 0.3*30)
        //         = 0.3*(0 + 3) + 0.7*(14 + 9)
        //         = 0.3*3 + 0.7*23
        //         = 0.9 + 16.1
        //         = 17.0
        assert!((result - 17.0).abs() < 1e-5);
        assert!(!outside);
    }

    #[test]
    fn test_interpolate_float_2d_outside() {
        // 2-D where one param is outside range.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 2, vec![0.0, 1.0]),
        ];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];
        let ctx = PivotContext {
            param_values: &[2.0, 0.5], // ParamA above range
            param_updated: &[false, false],
            param_ids: &[make_id("ParamA"), make_id("ParamB")],
            init_version: 0,
            setup_required: false,
        };
        let pivot_values = vec![0.0f32, 10.0f32, 20.0f32, 30.0f32];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (_result, outside) = interpolate_float(
            &[0, 1], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        assert!(outside, "should mark outside when param exceeds range");
    }

    // ── interpolate_int tests ────────────────────────────────────

    #[test]
    fn test_interpolate_int_1d() {
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.3],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let pivot_values = vec![10i32, 20i32];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (result, _outside) = interpolate_int(
            &[0], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        // 10*0.7 + 20*0.3 = 7+6 = 13 → int = 13
        assert_eq!(result, 13);
    }

    // ── interpolate_points tests ─────────────────────────────────

    #[test]
    fn test_interpolate_points_0d() {
        // dim_count=0: copy first corner's data directly.
        let pivots = vec![make_pivot("ParamA", 1, vec![0.5])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        // 2 corners × 4 floats per corner (2 points)
        let pivot_points = vec![
            1.0, 2.0, 3.0, 4.0,   // corner 0
            5.0, 6.0, 7.0, 8.0,   // corner 1
        ];
        let mut dst = vec![0.0f32; 4];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let outside = interpolate_points(
            &[0], &pivots, &mut states, &ctx, &pivot_points,
            4, // points_per_corner
            2, // num_points (2 × 2 = 4 coords)
            &mut dst, &mut indices, &mut t,
        );

        assert!(!outside);
        // Should copy corner 0 (dims=0 → tmp_indices[0]=0)
        assert!((dst[0] - 1.0).abs() < 1e-6);
        assert!((dst[1] - 2.0).abs() < 1e-6);
        assert!((dst[2] - 3.0).abs() < 1e-6);
        assert!((dst[3] - 4.0).abs() < 1e-6);
    }

    #[test]
    fn test_interpolate_points_1d() {
        // 1-D interpolation between two point arrays.
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.3],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        // 2 corners × 4 floats each (2 points = 4 coords)
        let pivot_points = vec![
            // corner 0: at pivot[0]
            10.0, 20.0, 30.0, 40.0,
            // corner 1: at pivot[1]
            100.0, 200.0, 300.0, 400.0,
        ];
        let mut dst = vec![0.0f32; 4];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let _outside = interpolate_points(
            &[0], &pivots, &mut states, &ctx, &pivot_points,
            4, 2, &mut dst, &mut indices, &mut t,
        );

        // Lerp each coord: val = corner0*(1-0.3) + corner1*0.3
        assert!((dst[0] - (10.0 * 0.7 + 100.0 * 0.3)).abs() < 1e-5);
        assert!((dst[1] - (20.0 * 0.7 + 200.0 * 0.3)).abs() < 1e-5);
        assert!((dst[2] - (30.0 * 0.7 + 300.0 * 0.3)).abs() < 1e-5);
        assert!((dst[3] - (40.0 * 0.7 + 400.0 * 0.3)).abs() < 1e-5);
    }

    #[test]
    fn test_interpolate_points_2d() {
        // 2-D bilinear interpolation of point arrays.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 2, vec![0.0, 1.0]),
        ];
        let ctx = PivotContext {
            param_values: &[0.3, 0.7],
            param_updated: &[false, false],
            param_ids: &[make_id("ParamA"), make_id("ParamB")],
            init_version: 0,
            setup_required: false,
        };
        // 4 corners, each with points_per_corner=4 floats (1 point = 2 coords, padded to 4)
        let pivot_points = vec![
            // corner 0 (V00): ParamA=0, ParamB=0
            1.0, 2.0, 0.0, 0.0,
            // corner 1 (V01): ParamA=1, ParamB=0
            3.0, 4.0, 0.0, 0.0,
            // corner 2 (V10): ParamA=0, ParamB=1
            5.0, 6.0, 0.0, 0.0,
            // corner 3 (V11): ParamA=1, ParamB=1
            7.0, 8.0, 0.0, 0.0,
        ];
        let mut dst = vec![0.0f32; 2];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];

        let _outside = interpolate_points(
            &[0, 1], &pivots, &mut states, &ctx, &pivot_points,
            4, // points_per_corner
            1, // 1 point (2 coords)
            &mut dst, &mut indices, &mut t,
        );

        // Weights: w(000)=(1-0.3)*(1-0.7)=0.21, w(001)=0.3*0.3=0.09,
        //          w(010)=0.7*0.7=0.49,           w(011)=0.3*0.7=0.21
        // Result x: 0.21*1 + 0.09*3 + 0.49*5 + 0.21*7 = 4.4
        // Result y: 0.21*2 + 0.09*4 + 0.49*6 + 0.21*8 = 5.4
        assert!((dst[0] - 4.4).abs() < 1e-5, "got {}", dst[0]);
        assert!((dst[1] - 5.4).abs() < 1e-5, "got {}", dst[1]);
    }

    // ── check_param_updated tests ────────────────────────────────

    #[test]
    fn test_check_param_updated_setup_required() {
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: true,
        };

        assert!(check_param_updated(&[0], &pivots, &mut states, &ctx));
    }

    #[test]
    fn test_check_param_updated_no_change() {
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };

        assert!(!check_param_updated(&[0], &pivots, &mut states, &ctx));
    }

    #[test]
    fn test_check_param_updated_changed() {
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];
        let ctx = PivotContext {
            param_values: &[0.5],
            param_updated: &[true],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };

        assert!(check_param_updated(&[0], &pivots, &mut states, &ctx));
    }

    #[test]
    fn test_check_param_updated_ignores_unrelated() {
        // Two params, only one updated.
        let pivots = vec![
            make_pivot("ParamA", 2, vec![0.0, 1.0]),
            make_pivot("ParamB", 2, vec![0.0, 1.0]),
        ];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];
        let ctx = PivotContext {
            param_values: &[0.5, 0.5],
            param_updated: &[true, false],
            param_ids: &[make_id("ParamB"), make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };

        // PivotManager owns ParamA (index 0 in ctx) and ParamB (index 1).
        // Only ParamA is updated, so the manager should see it.
        assert!(check_param_updated(&[0, 1], &pivots, &mut states, &ctx));
    }

    #[test]
    fn test_calc_pivot_values_version_cache() {
        // Verify that param_index cache works across versions.
        let pivots = vec![make_pivot("ParamA", 2, vec![0.0, 1.0])];
        let mut states = vec![ParamPivotState::default()];

        // Frame 1: resolve and compute.
        let ctx1 = PivotContext {
            param_values: &[0.3],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside1 = false;
        calc_pivot_values(&[0], &pivots, &mut states, &ctx1, &mut outside1);

        // State should be cached for version 0.
        assert_eq!(states[0].param_index, 0);
        assert_eq!(states[0].init_version, 0);

        // Frame 2: same version → cache hit (should not re-resolve).
        let ctx2 = PivotContext {
            param_values: &[0.7],
            param_updated: &[false],
            param_ids: &[make_id("ParamA")],
            init_version: 0,
            setup_required: false,
        };
        let mut outside2 = false;
        let _ = calc_pivot_values(&[0], &pivots, &mut states, &ctx2, &mut outside2);
        // State unchanged because same init_version.
        assert_eq!(states[0].param_index, 0);
        assert_eq!(states[0].init_version, 0);
    }

    #[test]
    fn test_interpolate_int_generic_case() {
        // Generic path with weight computation (used when dim_count >= 5).
        // We'll create 3 dims to test the combinatorial weight generation.
        let pivots = vec![
            make_pivot("P1", 2, vec![0.0, 1.0]),
            make_pivot("P2", 2, vec![0.0, 1.0]),
            make_pivot("P3", 2, vec![0.0, 1.0]),
        ];
        let mut states = vec![
            ParamPivotState::default(),
            ParamPivotState::default(),
            ParamPivotState::default(),
        ];
        let ctx = PivotContext {
            param_values: &[0.2, 0.4, 0.6],
            param_updated: &[false, false, false],
            param_ids: &[make_id("P1"), make_id("P2"), make_id("P3")],
            init_version: 0,
            setup_required: false,
        };
        // 8 corner values
        let pivot_values = vec![
            1000i32, 2000, 3000, 4000, 5000, 6000, 7000, 8000,
        ];
        let mut indices = [0u16; PIVOT_TABLE_SIZE];
        let mut t = [0.0f32; MAX_INTERPOLATION];

        let (result, _outside) = interpolate_int(
            &[0, 1, 2], &pivots, &mut states, &ctx, &pivot_values,
            &mut indices, &mut t,
        );

        // Trilinear: weight for each of 8 corners = product of t/(1-t) per dim.
        // P1 t=0.2, P2 t=0.4, P3 t=0.6
        // Corner 000: (1-0.2)*(1-0.4)*(1-0.6) = 0.8*0.6*0.4 = 0.192 → V[0]=1000
        // Corner 001: 0.2*0.6*0.4 = 0.048 → V[1]=2000
        // Corner 010: 0.8*0.4*0.4 = 0.128 → V[2]=3000
        // Corner 011: 0.2*0.4*0.4 = 0.032 → V[3]=4000
        // Corner 100: 0.8*0.6*0.6 = 0.288 → V[4]=5000
        // Corner 101: 0.2*0.6*0.6 = 0.072 → V[5]=6000
        // Corner 110: 0.8*0.4*0.6 = 0.192 → V[6]=7000
        // Corner 111: 0.2*0.4*0.6 = 0.048 → V[7]=8000
        // Sum: 0.192*1000 + 0.048*2000 + 0.128*3000 + 0.032*4000 + 0.288*5000 + 0.072*6000 + 0.192*7000 + 0.048*8000
        // = 192 + 96 + 384 + 128 + 1440 + 432 + 1344 + 384
        // = 4400
        assert_eq!(result, 4400);
    }
}
