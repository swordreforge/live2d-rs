//! Deformer interpolation engine (Phase 3).
//!
//! Implements:
//! - **WarpDeformer** — grid-based Free-Form Deformation via bilinear
//!   interpolation within a `(row+1)×(col+1)` control-point grid.
//! - **RotationDeformer** — affine transform interpolation chained through
//!   a parent deformer hierarchy, with rotation accumulation.
//!
//! Reference: `live2d/core/deformer/warp_deformer.py`,
//!            `live2d/core/deformer/roation_deformer.py`,
//!            `live2d/core/deformer/deformer.py`,
//!            `live2d/core/deformer/deformer_context.py`

use super::types::{AffineEnt, Deformer, DeformerKind, Drawable, Id, ParamPivot};
use crate::moc2::pivot::{
    calc_pivot_indices, calc_pivot_values, check_param_updated, interpolate_float,
    interpolate_int, ParamPivotState, PivotContext,
};
use std::f32::consts::PI;

// ── constants ──────────────────────────────────────────────────────

pub(crate) const DEG_TO_RAD: f32 = PI / 180.0;
pub(crate) const RAD_TO_DEG: f32 = 180.0 / PI;
pub(crate) const DEFORMER_INDEX_NOT_INIT: i32 = -2;

/// Deformer type identifiers (matches Python Deformer.TYPE_*).
pub(crate) const TYPE_ROTATION: i32 = 1;
pub(crate) const TYPE_WARP: i32 = 2;

// ── base context (shared by WarpContext + RotationContext) ─────────

/// Base deformer context — common runtime state for any deformer.
///
/// Reference: `DeformerContext` in `deformer_context.py`
#[derive(Debug, Clone)]
pub(crate) struct DeformerContext {
    pub parts_index: Option<usize>,
    pub outside_param: bool,
    pub available: bool,
    pub total_scale: f32,
    pub interpolated_opacity: f32,
    pub total_opacity: f32,
}

impl Default for DeformerContext {
    fn default() -> Self {
        Self {
            parts_index: None,
            outside_param: false,
            available: true,
            total_scale: 1.0,
            interpolated_opacity: 1.0,
            total_opacity: 1.0,
        }
    }
}

impl DeformerContext {
    pub fn is_available(&self) -> bool {
        self.available && !self.outside_param
    }

    pub fn set_available(&mut self, value: bool) {
        self.available = value;
    }

    pub fn is_outside_param(&self) -> bool {
        self.outside_param
    }

    pub fn set_outside_param(&mut self, value: bool) {
        self.outside_param = value;
    }

    pub fn get_total_scale(&self) -> f32 {
        self.total_scale
    }

    pub fn set_total_scale(&mut self, value: f32) {
        self.total_scale = value;
    }

    pub fn get_interpolated_opacity(&self) -> f32 {
        self.interpolated_opacity
    }

    pub fn set_interpolated_opacity(&mut self, value: f32) {
        self.interpolated_opacity = value;
    }

    pub fn get_total_opacity(&self) -> f32 {
        self.total_opacity
    }

    pub fn set_total_opacity(&mut self, value: f32) {
        self.total_opacity = value;
    }
}

// ── WarpContext ────────────────────────────────────────────────────

/// Runtime state for a WarpDeformer.
///
/// Reference: `WarpContext` in `warp_context.py`
#[derive(Debug, Clone)]
pub(crate) struct WarpContext {
    pub base: DeformerContext,
    pub tmp_deformer_index: i32,
    pub interpolated_points: Vec<f32>,
    pub transformed_points: Option<Vec<f32>>,
}

impl WarpContext {
    /// Allocate a new WarpContext for a warp deformer with a `grid_size × 2`
    /// float buffer (where `grid_size = (row+1)*(col+1)`).
    pub fn new(need_transform: bool, grid_size: usize) -> Self {
        let buf_size = grid_size * 2;
        Self {
            base: DeformerContext::default(),
            tmp_deformer_index: DEFORMER_INDEX_NOT_INIT,
            interpolated_points: vec![0.0f32; buf_size],
            transformed_points: if need_transform {
                Some(vec![0.0f32; buf_size])
            } else {
                None
            },
        }
    }
}

// ── RotationContext ────────────────────────────────────────────────

/// Runtime state for a RotationDeformer.
///
/// Reference: `RotationContext` in `rotation_context.py`
#[derive(Debug, Clone)]
pub(crate) struct RotationContext {
    pub base: DeformerContext,
    pub tmp_deformer_index: i32,
    pub interpolated_affine: AffineEnt,
    pub transformed_affine: Option<AffineEnt>,
}

impl RotationContext {
    pub fn new(need_transform: bool) -> Self {
        Self {
            base: DeformerContext::default(),
            tmp_deformer_index: DEFORMER_INDEX_NOT_INIT,
            interpolated_affine: AffineEnt::ZERO,
            transformed_affine: if need_transform {
                Some(AffineEnt::ZERO)
            } else {
                None
            },
        }
    }
}

// ── math helpers ───────────────────────────────────────────────────

/// Compute the signed angle difference (in radians) between `q1` and `q2`,
/// normalised to `[-π, π]`.
///
/// Reference: `UtMath.getAngleDiff`
pub(crate) fn get_angle_diff(q1: f32, q2: f32) -> f32 {
    let mut t = q1 - q2;
    while t < -PI {
        t += 2.0 * PI;
    }
    while t > PI {
        t -= 2.0 * PI;
    }
    t
}

/// Compute the signed angle from `v1` to `v2`.
///
/// Reference: `UtMath.getAngleNotAbs`
pub(crate) fn get_angle_not_abs(v1: (f32, f32), v2: (f32, f32)) -> f32 {
    let q1 = v1.1.atan2(v1.0);
    let q2 = v2.1.atan2(v2.0);
    get_angle_diff(q1, q2)
}

// ── deformer base helpers ──────────────────────────────────────────

/// Check whether a deformer needs to chain-transform through its parent.
///
/// Returns `true` when `target_id` is not the base (root) id.
///
/// Reference: `Deformer.needTransform`
pub(crate) fn deformer_need_transform(target_id: &Id) -> bool {
    !is_base_id(target_id)
}

/// Check if an ID is the base (root) target.
fn is_base_id(id: &Id) -> bool {
    id.is_empty()
        || id.as_ref() == "DST_BASE"
        || id.as_ref() == "BASE"
}

/// Interpolate opacity for a deformer via its pivot manager.
///
/// Reference: `Deformer.interpolateOpacity`
pub(crate) fn interpolate_opacity(
    pivot_opacities: &[f32],
    param_pivot_indices: &[usize],
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) -> f32 {
    if pivot_opacities.is_empty() {
        return 1.0;
    }

    let (opacity, _outside) = interpolate_float(
        param_pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        pivot_opacities,
        tmp_indices,
        tmp_t,
    );
    opacity
}

// ── helpers ────────────────────────────────────────────────────────

/// Number of control points in a warp deformer's grid.
pub(crate) fn warp_point_count(row: i32, col: i32) -> i32 {
    (row + 1) * (col + 1)
}

/// Build deformer tree: parent indices + topological order.
///
/// A deformer whose `target_id` is empty / "DST_BASE" / "BASE" is a root.
/// Otherwise, the parent is the deformer whose `id` matches `target_id`.
/// Returns `(parents, order)` where `parents[i] = Some(j)` if deformer `i`
/// has parent `j`, and `order` is a topological ordering (parents before
/// children).
pub(crate) fn build_deformer_tree(
    deformers: &[Deformer],
) -> (Vec<Option<usize>>, Vec<usize>) {
    let count = deformers.len();
    let mut parents = vec![None; count];

    // Build id → index lookup
    let id_to_idx: std::collections::HashMap<&str, usize> = deformers
        .iter()
        .enumerate()
        .map(|(i, d)| (d.id.as_ref(), i))
        .collect();

    // Find parent for each deformer
    for (i, def) in deformers.iter().enumerate() {
        if !deformer_need_transform(&def.target_id) {
            continue; // root deformer
        }
        if let Some(&parent_idx) = id_to_idx.get(def.target_id.as_ref()) {
            parents[i] = Some(parent_idx);
        }
    }

    // Topological sort via Kahn's algorithm
    let mut in_degree = vec![0usize; count];
    for i in 0..count {
        if parents[i].is_some() {
            in_degree[i] += 1;
        }
    }

    let mut queue: Vec<usize> = (0..count).filter(|&i| in_degree[i] == 0).collect();
    let mut order = Vec::with_capacity(count);

    while let Some(idx) = queue.pop() {
        order.push(idx);
        for child in 0..count {
            if parents[child] == Some(idx) {
                in_degree[child] = in_degree[child].saturating_sub(1);
                if in_degree[child] == 0 {
                    queue.push(child);
                }
            }
        }
    }

    (parents, order)
}

/// Returns `TYPE_WARP` or `TYPE_ROTATION` for a deformer.
pub(crate) fn deformer_get_type(deformer: &Deformer) -> i32 {
    match deformer.kind {
        DeformerKind::Warp { .. } => TYPE_WARP,
        DeformerKind::Rotation { .. } => TYPE_ROTATION,
    }
}

/// Grid index helper: x of control-point `(r, c)` in row-major grid.
#[inline]
fn gx(grid: &[f32], r: i32, c: i32, a1: i32) -> f32 {
    grid[((r) + (c) * a1) as usize * 2]
}

/// Grid index helper: y of control-point `(r, c)` in row-major grid.
#[inline]
fn gy(grid: &[f32], r: i32, c: i32, a1: i32) -> f32 {
    grid[((r) + (c) * a1) as usize * 2 + 1]
}

// ── WarpDeformer pure functions ────────────────────────────────────

/// Set up grid interpolation for a WarpDeformer.
///
/// If no relevant parameter changed, returns early (no-op).
/// Otherwise interpolates the pivot grid points and opacity.
///
/// Reference: `WarpDeformer.setupInterpolate`
#[allow(clippy::too_many_arguments)]
pub(crate) fn warp_setup_interpolate(
    deformer: &Deformer,
    context: &mut WarpContext,
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
) {
    if !check_param_updated(
        &[deformer.pivot_manager_index],
        param_pivots,
        pivot_states,
        ctx,
    ) {
        return;
    }

    let pivot_arrays = match &deformer.kind {
        DeformerKind::Warp { pivot_points, .. } => pivot_points.as_slice(),
        _ => return,
    };

    let (row, col) = match &deformer.kind {
        DeformerKind::Warp { row, col, .. } => (*row, *col),
        _ => return,
    };

    let point_count = warp_point_count(row, col) as usize;

    // Interpolate pivot grid points.
    let mut outside = false;
    let dim_count = calc_pivot_values(
        &[deformer.pivot_manager_index],
        param_pivots,
        pivot_states,
        ctx,
        &mut outside,
    );

    if dim_count == 0 {
        let len = point_count * 2;
        context.interpolated_points[..len].copy_from_slice(&pivot_arrays[0][..len]);
    } else {
        calc_pivot_indices(
            &[deformer.pivot_manager_index],
            pivot_states,
            param_pivots,
            dim_count,
            tmp_indices,
            tmp_t,
        );

        let num_vertices = 1usize << dim_count;
        let coord_count = point_count * 2;

        let mut weights = [0.0f32; 64];
        debug_assert!(num_vertices <= weights.len());
        for v in 0..num_vertices {
            weights[v] = weight_for_vertex(v, tmp_t, dim_count);
        }

        for ci in 0..coord_count {
            let mut sum = 0.0f32;
            for v in 0..num_vertices {
                let corner = tmp_indices[v] as usize;
                sum += weights[v] * pivot_arrays[corner][ci];
            }
            context.interpolated_points[ci] = sum;
        }
    }

    context.base.set_outside_param(outside);

    // Interpolate opacity.
    let opacity = interpolate_opacity(
        &deformer.pivot_opacities,
        &[deformer.pivot_manager_index],
        param_pivots,
        pivot_states,
        ctx,
        tmp_indices,
        tmp_t,
    );
    context.base.set_interpolated_opacity(opacity);
}

/// Chain-transform a warp deformer's interpolated grid through the
/// parent deformer.
///
/// For root deformers (`need_transform == false`): no-op — the engine
/// falls back to `interpolated_points` automatically.
///
/// For child deformers: transforms every grid control point through
/// the parent's `transform_fn`.
///
/// Reference: `WarpDeformer.setupTransform`
pub(crate) fn warp_setup_transform(
    context: &mut WarpContext,
    need_transform: bool,
    parent_idx: Option<usize>,
    transform_fn: &dyn Fn(usize, &[f32], &mut [f32], i32, i32, i32),
) {
    if !need_transform {
        return;
    }
    let Some(parent) = parent_idx else { return };
    let Some(transformed) = context.transformed_points.as_mut() else { return };

    let num_points = (context.interpolated_points.len() / 2) as i32;
    transform_fn(
        parent,
        &context.interpolated_points,
        transformed,
        num_points,
        0,
        2,
    );
}

/// Compute the multi-linear blend weight for `vertex_index` (a binary
/// code) given `t` values for each dimension.
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

/// Transform points through a WarpDeformer.
///
/// Uses the transformed (or interpolated) grid control points to
/// deform each source vertex via the SDK2 warp algorithm.
///
/// Reference: `WarpDeformer.transformPoints`
pub(crate) fn warp_transform_points(
    deformer: &Deformer,
    context: &WarpContext,
    src_points: &[f32],
    dst_points: &mut [f32],
    num_point: i32,
    pt_offset: i32,
    pt_step: i32,
) {
    let (row, col) = match &deformer.kind {
        DeformerKind::Warp { row, col, .. } => (*row, *col),
        _ => return,
    };

    let grid = context
        .transformed_points
        .as_deref()
        .unwrap_or(&context.interpolated_points);

    warp_transform_points_sdk2(
        src_points, dst_points, num_point, pt_offset, pt_step, grid, row, col,
    );
}

/// Core SDK2 warp transform — bilinear interpolation within a
/// `(row+1)×(col+1)` control-point grid.
///
/// Input vertices carry "UV" coordinates in `[0, row] × [0, col]` space.
/// - **In-bounds**: bilinear interpolation within the containing cell.
/// - **Out-of-bounds**: extrapolation using nearest edge/corner + average
///   corner gradients.
///
/// Reference: `WarpDeformer.transformPoints_sdk2` (static method)
// Variable names match the Python reference for direct comparison.
#[allow(clippy::too_many_arguments, non_snake_case)]
pub(crate) fn warp_transform_points_sdk2(
    src: &[f32],
    dst: &mut [f32],
    point_count: i32,
    src_offset: i32,
    src_step: i32,
    grid: &[f32],
    row: i32,
    col: i32,
) {
    let end = point_count * src_step;
    let a1 = row + 1; // grid width (number of control points per row)

    // Pre-computed corner info (lazy, computed once when any out-of-bounds vertex is hit).
    let mut corner_avg_x = 0.0f32;
    let mut corner_avg_y = 0.0f32;
    let mut bl = 0.0f32;
    let mut bk = 0.0f32;
    let mut bf = 0.0f32;
    let mut be = 0.0f32;
    let mut corner_info_initialized = false;

    for ba in (src_offset..end).step_by(src_step as usize) {
        let u = src[ba as usize];
        let v = src[ba as usize + 1];

        let bd = u * row as f32;
        let a7 = v * col as f32;

        if bd < 0.0 || a7 < 0.0 || bd >= row as f32 || a7 >= col as f32 {
            // ── Out of bounds ──
            if !corner_info_initialized {
                corner_info_initialized = true;

                corner_avg_x = 0.25
                    * (gx(grid, 0, 0, a1)
                        + gx(grid, row, 0, a1)
                        + gx(grid, 0, col, a1)
                        + gx(grid, row, col, a1));
                corner_avg_y = 0.25
                    * (gy(grid, 0, 0, a1)
                        + gy(grid, row, 0, a1)
                        + gy(grid, 0, col, a1)
                        + gy(grid, row, col, a1));

                let aM = gx(grid, row, col, a1) - gx(grid, 0, 0, a1);
                let aL = gy(grid, row, col, a1) - gy(grid, 0, 0, a1);
                let bh = gx(grid, row, 0, a1) - gx(grid, 0, col, a1);
                let bg = gy(grid, row, 0, a1) - gy(grid, 0, col, a1);

                bl = (aM + bh) * 0.5;
                bk = (aL + bg) * 0.5;
                bf = (aM - bh) * 0.5;
                be = (aL - bg) * 0.5;

                corner_avg_x -= 0.5 * (bl + bf);
                corner_avg_y -= 0.5 * (bk + be);
            }

            let near_grid = (-2.0 < u && u < 3.0) && (-2.0 < v && v < 3.0);

            if !near_grid {
                // Far outside — linear extrapolation.
                dst[ba as usize] = corner_avg_x + u * bl + v * bf;
                dst[ba as usize + 1] = corner_avg_y + u * bk + v * be;
                continue;
            }

            if u <= 0.0 {
                if v <= 0.0 {
                    // Top-left corner region.
                    let a3 = gx(grid, 0, 0, a1);
                    let a2 = gy(grid, 0, 0, a1);
                    let a8 = corner_avg_x - 2.0 * bl;
                    let a6 = corner_avg_y - 2.0 * bk;
                    let aK = corner_avg_x - 2.0 * bf;
                    let aJ = corner_avg_y - 2.0 * be;
                    let aO = corner_avg_x - 2.0 * bl - 2.0 * bf;
                    let aN = corner_avg_y - 2.0 * bk - 2.0 * be;
                    let bj = 0.5 * (u - (-2.0));
                    let bi = 0.5 * (v - (-2.0));
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                } else if v >= 1.0 {
                    // Bottom-left corner region.
                    let aK = gx(grid, 0, col, a1);
                    let aJ = gy(grid, 0, col, a1);
                    let aO = corner_avg_x - 2.0 * bl + 1.0 * bf;
                    let aN = corner_avg_y - 2.0 * bk + 1.0 * be;
                    let a3 = corner_avg_x + 3.0 * bf;
                    let a2 = corner_avg_y + 3.0 * be;
                    let a8 = corner_avg_x - 2.0 * bl + 3.0 * bf;
                    let a6 = corner_avg_y - 2.0 * bk + 3.0 * be;
                    let bj = 0.5 * (u - (-2.0));
                    let bi = 0.5 * (v - 1.0);
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                } else {
                    // Left edge (between corners).
                    let mut aH = a7 as i32;
                    if aH == col {
                        aH = col - 1;
                    }
                    let bj = 0.5 * (u - (-2.0));
                    let bi = a7 - aH as f32;
                    let bb = aH as f32 / col as f32;
                    let a9 = (aH + 1) as f32 / col as f32;
                    let aK = gx(grid, 0, aH, a1);
                    let aJ = gy(grid, 0, aH, a1);
                    let a3 = gx(grid, 0, aH + 1, a1);
                    let a2 = gy(grid, 0, aH + 1, a1);
                    let aO = corner_avg_x - 2.0 * bl + bb * bf;
                    let aN = corner_avg_y - 2.0 * bk + bb * be;
                    let a8 = corner_avg_x - 2.0 * bl + a9 * bf;
                    let a6 = corner_avg_y - 2.0 * bk + a9 * be;
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                }
            } else if u >= 1.0 {
                if v <= 0.0 {
                    // Top-right corner region.
                    let a8 = gx(grid, row, 0, a1);
                    let a6 = gy(grid, row, 0, a1);
                    let a3 = corner_avg_x + 3.0 * bl;
                    let a2 = corner_avg_y + 3.0 * bk;
                    let aO = corner_avg_x + 1.0 * bl - 2.0 * bf;
                    let aN = corner_avg_y + 1.0 * bk - 2.0 * be;
                    let aK = corner_avg_x + 3.0 * bl - 2.0 * bf;
                    let aJ = corner_avg_y + 3.0 * bk - 2.0 * be;
                    let bj = 0.5 * (u - 1.0);
                    let bi = 0.5 * (v - (-2.0));
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                } else if v >= 1.0 {
                    // Bottom-right corner region.
                    let aO = gx(grid, row, col, a1);
                    let aN = gy(grid, row, col, a1);
                    let aK = corner_avg_x + 3.0 * bl + 1.0 * bf;
                    let aJ = corner_avg_y + 3.0 * bk + 1.0 * be;
                    let a8 = corner_avg_x + 1.0 * bl + 3.0 * bf;
                    let a6 = corner_avg_y + 1.0 * bk + 3.0 * be;
                    let a3 = corner_avg_x + 3.0 * bl + 3.0 * bf;
                    let a2 = corner_avg_y + 3.0 * bk + 3.0 * be;
                    let bj = 0.5 * (u - 1.0);
                    let bi = 0.5 * (v - 1.0);
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                } else {
                    // Right edge.
                    let mut aH = a7 as i32;
                    if aH == col {
                        aH = col - 1;
                    }
                    let bj = 0.5 * (u - 1.0);
                    let bi = a7 - aH as f32;
                    let bb = aH as f32 / col as f32;
                    let a9 = (aH + 1) as f32 / col as f32;
                    let aO = gx(grid, row, aH, a1);
                    let aN = gy(grid, row, aH, a1);
                    let a8 = gx(grid, row, aH + 1, a1);
                    let a6 = gy(grid, row, aH + 1, a1);
                    let aK = corner_avg_x + 3.0 * bl + bb * bf;
                    let aJ = corner_avg_y + 3.0 * bk + bb * be;
                    let a3 = corner_avg_x + 3.0 * bl + a9 * bf;
                    let a2 = corner_avg_y + 3.0 * bk + a9 * be;
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                }
            } else {
                // u in (0, 1) — row interior, but v out of bounds.
                if v <= 0.0 {
                    // Top edge.
                    let mut aY = bd as i32;
                    if aY == row {
                        aY = row - 1;
                    }
                    let bj = bd - aY as f32;
                    let bi = 0.5 * (v - (-2.0));
                    let bp = aY as f32 / row as f32;
                    let bo = (aY + 1) as f32 / row as f32;
                    let a8 = gx(grid, aY, 0, a1);
                    let a6 = gy(grid, aY, 0, a1);
                    let a3 = gx(grid, aY + 1, 0, a1);
                    let a2 = gy(grid, aY + 1, 0, a1);
                    let aO = corner_avg_x + bp * bl - 2.0 * bf;
                    let aN = corner_avg_y + bp * bk - 2.0 * be;
                    let aK = corner_avg_x + bo * bl - 2.0 * bf;
                    let aJ = corner_avg_y + bo * bk - 2.0 * be;
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                } else {
                    // v >= 1.0 — Bottom edge.
                    let mut aY = bd as i32;
                    if aY == row {
                        aY = row - 1;
                    }
                    let bj = bd - aY as f32;
                    let bi = 0.5 * (v - 1.0);
                    let bp = aY as f32 / row as f32;
                    let bo = (aY + 1) as f32 / row as f32;
                    let aO = gx(grid, aY, col, a1);
                    let aN = gy(grid, aY, col, a1);
                    let aK = gx(grid, aY + 1, col, a1);
                    let aJ = gy(grid, aY + 1, col, a1);
                    let a8 = corner_avg_x + bp * bl + 3.0 * bf;
                    let a6 = corner_avg_y + bp * bk + 3.0 * be;
                    let a3 = corner_avg_x + bo * bl + 3.0 * bf;
                    let a2 = corner_avg_y + bo * bk + 3.0 * be;
                    if bj + bi <= 1.0 {
                        dst[ba as usize] = aO + (aK - aO) * bj + (a8 - aO) * bi;
                        dst[ba as usize + 1] = aN + (aJ - aN) * bj + (a6 - aN) * bi;
                    } else {
                        dst[ba as usize] = a3 + (a8 - a3) * (1.0 - bj) + (aK - a3) * (1.0 - bi);
                        dst[ba as usize + 1] = a2 + (a6 - a2) * (1.0 - bj) + (aJ - a2) * (1.0 - bi);
                    }
                }
            }
        } else {
            // ── In bounds — standard bilinear interpolation ──
            let bn = bd - bd.floor();
            let bm = a7 - a7.floor();
            let ii = bd as i32;
            let jj = a7 as i32;
            let aV = 2 * (ii + jj * a1);

            if bn + bm < 1.0 {
                dst[ba as usize] = grid[aV as usize] * (1.0 - bn - bm)
                    + grid[aV as usize + 2] * bn
                    + grid[(aV + 2 * a1) as usize] * bm;
                dst[ba as usize + 1] = grid[aV as usize + 1] * (1.0 - bn - bm)
                    + grid[aV as usize + 3] * bn
                    + grid[(aV + 2 * a1 + 1) as usize] * bm;
            } else {
                dst[ba as usize] =
                    grid[(aV + 2 * a1 + 2) as usize] * (bn - 1.0 + bm)
                        + grid[(aV + 2 * a1) as usize] * (1.0 - bn)
                        + grid[aV as usize + 2] * (1.0 - bm);
                dst[ba as usize + 1] =
                    grid[(aV + 2 * a1 + 3) as usize] * (bn - 1.0 + bm)
                        + grid[(aV + 2 * a1 + 1) as usize] * (1.0 - bn)
                        + grid[aV as usize + 3] * (1.0 - bm);
            }
        }
    }
}

// ── RotationDeformer pure functions ────────────────────────────────

/// Set up interpolation for a RotationDeformer.
///
/// Uses the pivot system to multi-linearly blend affine keyframes.
/// Handles 0–4 dimensions with unrolled cases, falling back to a
/// generic weighted-sum approach for 5+ dimensions.
///
/// Reference: `RotationDeformer.setupInterpolate`
// Variable names match Python reference.
#[allow(clippy::too_many_arguments, non_snake_case)]
pub(crate) fn rotation_setup_interpolate(
    deformer: &Deformer,
    context: &mut RotationContext,
    param_pivots: &[ParamPivot],
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
    affines: &[AffineEnt],
) {
    let pivot_indices = &[deformer.pivot_manager_index];

    if !check_param_updated(pivot_indices, param_pivots, pivot_states, ctx) {
        return;
    }

    let mut outside = false;
    let dim_count = calc_pivot_values(pivot_indices, param_pivots, pivot_states, ctx, &mut outside);
    context.base.set_outside_param(outside);

    // Interpolate opacity (uses same pivot manager but different value array).
    let opacity = interpolate_opacity(
        &deformer.pivot_opacities,
        pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        tmp_indices,
        tmp_t,
    );
    context.base.set_interpolated_opacity(opacity);

    calc_pivot_indices(pivot_indices, pivot_states, param_pivots, dim_count, tmp_indices, tmp_t);

    let affine_indices = match &deformer.kind {
        DeformerKind::Rotation { affine_indices } => affine_indices.as_slice(),
        _ => return,
    };

    if dim_count == 0 {
        let src = &affines[affine_indices[tmp_indices[0] as usize]];
        context.interpolated_affine = *src;
    } else if dim_count == 1 {
        let a0 = &affines[affine_indices[tmp_indices[0] as usize]];
        let a1 = &affines[affine_indices[tmp_indices[1] as usize]];
        let t0 = tmp_t[0];
        context.interpolated_affine.origin_x = a0.origin_x + (a1.origin_x - a0.origin_x) * t0;
        context.interpolated_affine.origin_y = a0.origin_y + (a1.origin_y - a0.origin_y) * t0;
        context.interpolated_affine.scale_x = a0.scale_x + (a1.scale_x - a0.scale_x) * t0;
        context.interpolated_affine.scale_y = a0.scale_y + (a1.scale_y - a0.scale_y) * t0;
        context.interpolated_affine.rotation_deg =
            a0.rotation_deg + (a1.rotation_deg - a0.rotation_deg) * t0;
    } else if dim_count == 2 {
        let a00 = &affines[affine_indices[tmp_indices[0] as usize]];
        let a01 = &affines[affine_indices[tmp_indices[1] as usize]];
        let a10 = &affines[affine_indices[tmp_indices[2] as usize]];
        let a11 = &affines[affine_indices[tmp_indices[3] as usize]];
        let t0 = tmp_t[0];
        let t1 = tmp_t[1];

        let tmp = a00.origin_x + (a01.origin_x - a00.origin_x) * t0;
        let tmp2 = a10.origin_x + (a11.origin_x - a10.origin_x) * t0;
        context.interpolated_affine.origin_x = tmp + (tmp2 - tmp) * t1;

        let tmp = a00.origin_y + (a01.origin_y - a00.origin_y) * t0;
        let tmp2 = a10.origin_y + (a11.origin_y - a10.origin_y) * t0;
        context.interpolated_affine.origin_y = tmp + (tmp2 - tmp) * t1;

        let tmp = a00.scale_x + (a01.scale_x - a00.scale_x) * t0;
        let tmp2 = a10.scale_x + (a11.scale_x - a10.scale_x) * t0;
        context.interpolated_affine.scale_x = tmp + (tmp2 - tmp) * t1;

        let tmp = a00.scale_y + (a01.scale_y - a00.scale_y) * t0;
        let tmp2 = a10.scale_y + (a11.scale_y - a10.scale_y) * t0;
        context.interpolated_affine.scale_y = tmp + (tmp2 - tmp) * t1;

        let tmp = a00.rotation_deg + (a01.rotation_deg - a00.rotation_deg) * t0;
        let tmp2 = a10.rotation_deg + (a11.rotation_deg - a10.rotation_deg) * t0;
        context.interpolated_affine.rotation_deg = tmp + (tmp2 - tmp) * t1;
    } else if dim_count == 3 {
        let a000 = &affines[affine_indices[tmp_indices[0] as usize]];
        let a001 = &affines[affine_indices[tmp_indices[1] as usize]];
        let a010 = &affines[affine_indices[tmp_indices[2] as usize]];
        let a011 = &affines[affine_indices[tmp_indices[3] as usize]];
        let a100 = &affines[affine_indices[tmp_indices[4] as usize]];
        let a101 = &affines[affine_indices[tmp_indices[5] as usize]];
        let a110 = &affines[affine_indices[tmp_indices[6] as usize]];
        let a111 = &affines[affine_indices[tmp_indices[7] as usize]];
        let t0 = tmp_t[0];
        let t1 = tmp_t[1];
        let t2 = tmp_t[2];

        context.interpolated_affine.origin_x = tri_lerp(
            a000.origin_x, a001.origin_x, a010.origin_x, a011.origin_x,
            a100.origin_x, a101.origin_x, a110.origin_x, a111.origin_x,
            t0, t1, t2,
        );
        context.interpolated_affine.origin_y = tri_lerp(
            a000.origin_y, a001.origin_y, a010.origin_y, a011.origin_y,
            a100.origin_y, a101.origin_y, a110.origin_y, a111.origin_y,
            t0, t1, t2,
        );
        context.interpolated_affine.scale_x = tri_lerp(
            a000.scale_x, a001.scale_x, a010.scale_x, a011.scale_x,
            a100.scale_x, a101.scale_x, a110.scale_x, a111.scale_x,
            t0, t1, t2,
        );
        context.interpolated_affine.scale_y = tri_lerp(
            a000.scale_y, a001.scale_y, a010.scale_y, a011.scale_y,
            a100.scale_y, a101.scale_y, a110.scale_y, a111.scale_y,
            t0, t1, t2,
        );
        context.interpolated_affine.rotation_deg = tri_lerp(
            a000.rotation_deg, a001.rotation_deg, a010.rotation_deg, a011.rotation_deg,
            a100.rotation_deg, a101.rotation_deg, a110.rotation_deg, a111.rotation_deg,
            t0, t1, t2,
        );
    } else if dim_count == 4 {
        quad_lerp_affine_field(
            &affines,
            affine_indices,
            tmp_indices,
            tmp_t,
            |a| a.origin_x,
            &mut context.interpolated_affine.origin_x,
        );
        quad_lerp_affine_field(
            &affines,
            affine_indices,
            tmp_indices,
            tmp_t,
            |a| a.origin_y,
            &mut context.interpolated_affine.origin_y,
        );
        quad_lerp_affine_field(
            &affines,
            affine_indices,
            tmp_indices,
            tmp_t,
            |a| a.scale_x,
            &mut context.interpolated_affine.scale_x,
        );
        quad_lerp_affine_field(
            &affines,
            affine_indices,
            tmp_indices,
            tmp_t,
            |a| a.scale_y,
            &mut context.interpolated_affine.scale_y,
        );
        quad_lerp_affine_field(
            &affines,
            affine_indices,
            tmp_indices,
            tmp_t,
            |a| a.rotation_deg,
            &mut context.interpolated_affine.rotation_deg,
        );
    } else {
        // 5+ dimensions — generic weighted-sum fallback.
        let v = 1usize << dim_count;
        let mut weights = [0.0f32; 64];
        for bk in 0..v {
            let mut aI = bk;
            let mut aH = 1.0f32;
            for aL in 0..dim_count {
                aH *= if aI % 2 == 0 {
                    1.0 - tmp_t[aL]
                } else {
                    tmp_t[aL]
                };
                aI /= 2;
            }
            weights[bk] = aH;
        }

        let mut ox = 0.0f32;
        let mut oy = 0.0f32;
        let mut sx = 0.0f32;
        let mut sy = 0.0f32;
        let mut rot = 0.0f32;
        for aU in 0..v {
            let aff = &affines[affine_indices[tmp_indices[aU] as usize]];
            let w = weights[aU];
            ox += w * aff.origin_x;
            oy += w * aff.origin_y;
            sx += w * aff.scale_x;
            sy += w * aff.scale_y;
            rot += w * aff.rotation_deg;
        }
        context.interpolated_affine.origin_x = ox;
        context.interpolated_affine.origin_y = oy;
        context.interpolated_affine.scale_x = sx;
        context.interpolated_affine.scale_y = sy;
        context.interpolated_affine.rotation_deg = rot;
    }

    // Reflect flags always taken from the first affine.
    let first_affine = &affines[affine_indices[tmp_indices[0] as usize]];
    context.interpolated_affine.reflect_x = first_affine.reflect_x;
    context.interpolated_affine.reflect_y = first_affine.reflect_y;
}

/// Chain-transform a rotation deformer's interpolated affine through
/// the parent deformer.
///
/// For root deformers: `transformed_affine = Some(interpolated_affine)`.
///
/// For child deformers: measures parent rotation with
/// `get_direction_on_dst`, computes composite affine with total
/// rotation accumulated from parent.
///
/// Reference: `RotationDeformer.setupTransform`
pub(crate) fn rotation_setup_transform(
    context: &mut RotationContext,
    need_transform: bool,
    parent_idx: Option<usize>,
    transform_fn: &dyn Fn(usize, &[f32], &mut [f32], i32, i32, i32),
) {
    if !need_transform {
        context.transformed_affine = Some(context.interpolated_affine);
        return;
    }
    let Some(parent) = parent_idx else {
        context.transformed_affine = Some(context.interpolated_affine);
        return;
    };

    // Measure parent rotation using iterative direction search.
    let src_origin = [
        context.interpolated_affine.origin_x,
        context.interpolated_affine.origin_y,
    ];
    let src_dir = [1.0f32, 0.0f32];
    let mut ret_dir = [0.0f32; 2];

    get_direction_on_dst(parent, &src_origin, &src_dir, &mut ret_dir, transform_fn);

    let angle = get_angle_not_abs((src_dir[0], src_dir[1]), (ret_dir[0], ret_dir[1]));

    let mut out = context.interpolated_affine;
    out.rotation_deg += angle * RAD_TO_DEG;
    context.transformed_affine = Some(out);
}

/// Interpolate drawable vertex positions, opacity, and draw order
/// through the drawable's pivot manager.
///
/// Returns `true` if any parameter was outside its defined range.
///
/// Reference: `Mesh.setupInterpolate`
#[allow(clippy::too_many_arguments)]
pub(crate) fn drawable_setup_interpolate(
    drawable: &Drawable,
    pivot_states: &mut [ParamPivotState],
    ctx: &PivotContext,
    param_pivots: &[ParamPivot],
    tmp_indices: &mut [u16],
    tmp_t: &mut [f32],
    out_vertices: &mut [f32],
    out_draw_order: &mut i32,
    out_opacity: &mut f32,
) -> bool {
    let pivot_indices = &[drawable.pivot_manager_index];
    let mut outside = false;

    if !check_param_updated(pivot_indices, param_pivots, pivot_states, ctx) {
        return false;
    }

    let dim_count = calc_pivot_values(
        pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        &mut outside,
    );

    // Interpolate vertices
    let vert_count = drawable.vertex_count as usize;
    let coord_count = vert_count * 2;
    let dst = &mut out_vertices[..coord_count];

    if dim_count == 0 {
        dst.copy_from_slice(&drawable.pivot_points[..coord_count]);
    } else {
        calc_pivot_indices(
            pivot_indices,
            pivot_states,
            param_pivots,
            dim_count,
            tmp_indices,
            tmp_t,
        );

        let num_corners = 1usize << dim_count;
        let points_per_corner = coord_count;
        let mut weights = [0.0f32; 64];
        for v in 0..num_corners {
            let mut w = 1.0f32;
            let mut bits = v;
            for d in 0..dim_count {
                w *= if bits & 1 == 0 { 1.0 - tmp_t[d] } else { tmp_t[d] };
                bits >>= 1;
            }
            weights[v] = w;
        }

        for ci in 0..coord_count {
            let mut sum = 0.0f32;
            for v in 0..num_corners {
                let corner = tmp_indices[v] as usize;
                sum += weights[v] * drawable.pivot_points[corner * points_per_corner + ci];
            }
            dst[ci] = sum;
        }
    }

    // Interpolate opacity
    *out_opacity = interpolate_opacity(
        &drawable.pivot_opacities,
        pivot_indices,
        param_pivots,
        pivot_states,
        ctx,
        tmp_indices,
        tmp_t,
    );

    // Interpolate draw order
    if drawable.pivot_draw_orders.is_empty() {
        *out_draw_order = drawable.average_draw_order;
    } else {
        let (order, _) = interpolate_int(
            pivot_indices,
            param_pivots,
            pivot_states,
            ctx,
            &drawable.pivot_draw_orders,
            tmp_indices,
            tmp_t,
        );
        *out_draw_order = order;
    }

    outside
}

/// Trilinear interpolation helper.
fn tri_lerp(
    v000: f32, v001: f32, v010: f32, v011: f32,
    v100: f32, v101: f32, v110: f32, v111: f32,
    t0: f32, t1: f32, t2: f32,
) -> f32 {
    let c0 = v000 + (v001 - v000) * t0;
    let c1 = v010 + (v011 - v010) * t0;
    let layer0 = c0 + (c1 - c0) * t1;
    let c2 = v100 + (v101 - v100) * t0;
    let c3 = v110 + (v111 - v110) * t0;
    let layer1 = c2 + (c3 - c2) * t1;
    layer0 + (layer1 - layer0) * t2
}

/// Quad-linear interpolation for one field of `AffineEnt`.
fn quad_lerp_affine_field(
    affines: &[AffineEnt],
    affine_indices: &[usize],
    tmp_indices: &[u16],
    tmp_t: &[f32],
    field: fn(&AffineEnt) -> f32,
    out: &mut f32,
) {
    let t0 = tmp_t[0];
    let t1 = tmp_t[1];
    let t2 = tmp_t[2];
    let t3 = tmp_t[3];

    let v = |i: usize| field(&affines[affine_indices[tmp_indices[i] as usize]]);

    let layer0 = tri_lerp(v(0), v(1), v(2), v(3), v(4), v(5), v(6), v(7), t0, t1, t2);
    let layer1 = tri_lerp(v(8), v(9), v(10), v(11), v(12), v(13), v(14), v(15), t0, t1, t2);
    *out = layer0 + (layer1 - layer0) * t3;
}

/// Transform points through a RotationDeformer's affine transform.
///
/// Reference: `RotationDeformer.transformPoints`
pub(crate) fn rotation_transform_points(
    context: &RotationContext,
    src_points: &[f32],
    dst_points: &mut [f32],
    num_point: i32,
    pt_offset: i32,
    pt_step: i32,
) {
    let aff = context
        .transformed_affine
        .as_ref()
        .unwrap_or(&context.interpolated_affine);

    let rot_rad = aff.rotation_deg * DEG_TO_RAD;
    let sin_rot = rot_rad.sin();
    let cos_rot = rot_rad.cos();

    let total_scale = context.base.get_total_scale();

    let reflect_x = if aff.reflect_x { -1.0 } else { 1.0 };
    let reflect_y = if aff.reflect_y { -1.0 } else { 1.0 };

    let a = cos_rot * total_scale * reflect_x;
    let b = -sin_rot * total_scale * reflect_y;
    let c = sin_rot * total_scale * reflect_x;
    let d = cos_rot * total_scale * reflect_y;

    let end = num_point * pt_step;
    for k in (pt_offset..end).step_by(pt_step as usize) {
        let idx = k as usize;
        let px = src_points[idx];
        let py = src_points[idx + 1];
        dst_points[idx] = a * px + b * py + aff.origin_x;
        dst_points[idx + 1] = c * px + d * py + aff.origin_y;
    }
}

/// Iterative search for a direction vector through the parent deformer
/// chain.  Used to measure the rotation introduced by a parent transform.
///
/// `parent_idx` is the index of the parent deformer in the model.
/// `transform_fn` is called as `transform_fn(parent_idx, src, dst, 1, 0, 2)`.
///
/// Reference: `RotationDeformer.getDirectionOnDst`
#[allow(clippy::too_many_arguments)]
pub(crate) fn get_direction_on_dst(
    parent_idx: usize,
    src_origin: &[f32; 2],
    src_dir: &[f32; 2],
    ret_dir: &mut [f32; 2],
    transform_fn: &dyn Fn(usize, &[f32], &mut [f32], i32, i32, i32),
) {
    let mut origin_dst = *src_origin;

    // Transform origin through parent.
    transform_fn(parent_idx, src_origin, &mut origin_dst, 1, 0, 2);

    let num_steps = 10;
    let mut step = 1.0f32;

    for _ in 0..num_steps {
        // Forward direction trial.
        let trial_src = [
            src_origin[0] + step * src_dir[0],
            src_origin[1] + step * src_dir[1],
        ];
        let mut trial_dst = [0.0f32; 2];
        transform_fn(parent_idx, &trial_src, &mut trial_dst, 1, 0, 2);
        trial_dst[0] -= origin_dst[0];
        trial_dst[1] -= origin_dst[1];

        if trial_dst[0] != 0.0 || trial_dst[1] != 0.0 {
            ret_dir[0] = trial_dst[0];
            ret_dir[1] = trial_dst[1];
            return;
        }

        // Negative direction trial.
        let trial_src2 = [
            src_origin[0] - step * src_dir[0],
            src_origin[1] - step * src_dir[1],
        ];
        let mut trial_dst2 = [0.0f32; 2];
        transform_fn(parent_idx, &trial_src2, &mut trial_dst2, 1, 0, 2);
        trial_dst2[0] -= origin_dst[0];
        trial_dst2[1] -= origin_dst[1];

        if trial_dst2[0] != 0.0 || trial_dst2[1] != 0.0 {
            ret_dir[0] = -trial_dst2[0];
            ret_dir[1] = -trial_dst2[1];
            return;
        }

        step *= 0.1;
    }
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

    // ── DeformerContext tests ─────────────────────────────────────

    #[test]
    fn test_deformer_context_default() {
        let ctx = DeformerContext::default();
        assert!(ctx.available);
        assert!(!ctx.outside_param);
        assert!((ctx.total_scale - 1.0).abs() < 1e-6);
        assert!((ctx.interpolated_opacity - 1.0).abs() < 1e-6);
        assert!((ctx.total_opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_deformer_context_availability() {
        let mut ctx = DeformerContext::default();
        assert!(ctx.is_available());
        ctx.outside_param = true;
        assert!(!ctx.is_available());
        ctx.set_available(false);
        assert!(!ctx.is_available());
        ctx.outside_param = false;
        assert!(!ctx.is_available());
    }

    // ── math helpers ──────────────────────────────────────────────

    #[test]
    fn test_get_angle_diff_identity() {
        let d = get_angle_diff(0.0, 0.0);
        assert!((d).abs() < 1e-6);
    }

    #[test]
    fn test_get_angle_diff_half_pi() {
        let d = get_angle_diff(std::f32::consts::FRAC_PI_2, 0.0);
        assert!((d - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn test_get_angle_diff_wrap() {
        let d = get_angle_diff(3.0 * PI, 0.0);
        assert!((d - PI).abs() < 1e-6, "got {}", d);
    }

    #[test]
    fn test_get_angle_not_abs_orthogonal() {
        // atan2(1,0)=0 for v1, atan2(0,1)=π/2 for v2.
        // getAngleDiff(0, π/2) = -π/2 (q1 - q2, normalized).
        let angle = get_angle_not_abs((1.0, 0.0), (0.0, 1.0));
        assert!((angle + std::f32::consts::FRAC_PI_2).abs() < 1e-6, "got {}", angle);
    }

    // ── WarpDeformer tests ────────────────────────────────────────

    fn make_square_grid_2x2() -> (i32, i32, Vec<f32>) {
        let row = 2i32;
        let col = 2i32;
        let mut grid = vec![0.0f32; ((row + 1) * (col + 1) * 2) as usize];
        for r in 0..=row {
            for c in 0..=col {
                let idx = ((r + c * (row + 1)) * 2) as usize;
                grid[idx] = r as f32 * 10.0;   // x
                grid[idx + 1] = c as f32 * 10.0; // y
            }
        }
        (row, col, grid)
    }

    #[test]
    fn test_warp_transform_sdk2_in_bounds_center() {
        let (row, col, grid) = make_square_grid_2x2();
        let src = vec![0.5, 0.5];
        let mut dst = vec![0.0f32; 2];

        warp_transform_points_sdk2(&src, &mut dst, 1, 0, 2, &grid, row, col);

        assert!((dst[0] - 10.0).abs() < 1e-5, "got {}", dst[0]);
        assert!((dst[1] - 10.0).abs() < 1e-5, "got {}", dst[1]);
    }

    #[test]
    fn test_warp_transform_sdk2_in_bounds_top_left() {
        let (row, col, grid) = make_square_grid_2x2();
        let src = vec![0.0, 0.0];
        let mut dst = vec![0.0f32; 2];

        warp_transform_points_sdk2(&src, &mut dst, 1, 0, 2, &grid, row, col);

        assert!((dst[0] - 0.0).abs() < 1e-5, "got {}", dst[0]);
        assert!((dst[1] - 0.0).abs() < 1e-5, "got {}", dst[1]);
    }

    #[test]
    fn test_warp_transform_sdk2_in_bounds_bottom_right() {
        let (row, col, grid) = make_square_grid_2x2();
        let src = vec![1.0, 1.0];
        let mut dst = vec![0.0f32; 2];

        warp_transform_points_sdk2(&src, &mut dst, 1, 0, 2, &grid, row, col);

        assert!((dst[0] - 20.0).abs() < 1e-5, "got {}", dst[0]);
        assert!((dst[1] - 20.0).abs() < 1e-5, "got {}", dst[1]);
    }

    #[test]
    fn test_warp_transform_sdk2_multi_point_strided() {
        let (row, col, grid) = make_square_grid_2x2();
        // 3 points interleaved at stride 3: (0,0), (0.5,0.5), (1,1)
        let mut src = vec![0.0f32; 9];
        src[0] = 0.0; src[1] = 0.0;
        src[3] = 0.5; src[4] = 0.5;
        src[6] = 1.0; src[7] = 1.0;
        let mut dst = vec![0.0f32; 9];

        warp_transform_points_sdk2(&src, &mut dst, 3, 0, 3, &grid, row, col);

        assert!((dst[0] - 0.0).abs() < 1e-5);
        assert!((dst[1] - 0.0).abs() < 1e-5);
        assert!((dst[3] - 10.0).abs() < 1e-5);
        assert!((dst[4] - 10.0).abs() < 1e-5);
        assert!((dst[6] - 20.0).abs() < 1e-5);
        assert!((dst[7] - 20.0).abs() < 1e-5);
    }

    #[test]
    fn test_warp_transform_sdk2_out_of_bounds_near() {
        let (row, col, grid) = make_square_grid_2x2();
        // Just outside the top-left corner at UV (-1.0, -1.0)
        let src = vec![-1.0, -1.0];
        let mut dst = vec![0.0f32; 2];

        warp_transform_points_sdk2(&src, &mut dst, 1, 0, 2, &grid, row, col);

        // Should not panic and produce a reasonable result.
        assert!(dst[0].is_finite());
        assert!(dst[1].is_finite());
    }

    #[test]
    fn test_warp_transform_sdk2_far_outside() {
        let (row, col, grid) = make_square_grid_2x2();
        // Far outside — linear extrapolation.
        let src = vec![5.0, 5.0];
        let mut dst = vec![0.0f32; 2];

        warp_transform_points_sdk2(&src, &mut dst, 1, 0, 2, &grid, row, col);

        // Should produce a finite extrapolated result.
        assert!(dst[0].is_finite());
        assert!(dst[1].is_finite());
    }

    // ── RotationDeformer tests ────────────────────────────────────

    fn identity_affine() -> AffineEnt {
        AffineEnt {
            origin_x: 0.0,
            origin_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation_deg: 0.0,
            reflect_x: false,
            reflect_y: false,
        }
    }

    #[test]
    fn test_rotation_transform_identity() {
        let mut ctx = RotationContext::new(false);
        ctx.interpolated_affine = identity_affine();
        ctx.base.set_total_scale(1.0);

        let src = vec![10.0, 20.0, 30.0, 40.0];
        let mut dst = vec![0.0f32; 4];

        rotation_transform_points(&ctx, &src, &mut dst, 2, 0, 2);

        assert!((dst[0] - 10.0).abs() < 1e-5);
        assert!((dst[1] - 20.0).abs() < 1e-5);
        assert!((dst[2] - 30.0).abs() < 1e-5);
        assert!((dst[3] - 40.0).abs() < 1e-5);
    }

    #[test]
    fn test_rotation_transform_translation() {
        let mut ctx = RotationContext::new(false);
        ctx.interpolated_affine = AffineEnt {
            origin_x: 100.0,
            origin_y: 200.0,
            ..identity_affine()
        };
        ctx.base.set_total_scale(1.0);

        let src = vec![10.0, 20.0];
        let mut dst = vec![0.0f32; 2];

        rotation_transform_points(&ctx, &src, &mut dst, 1, 0, 2);

        assert!((dst[0] - 110.0).abs() < 1e-5);
        assert!((dst[1] - 220.0).abs() < 1e-5);
    }

    #[test]
    fn test_rotation_transform_scale() {
        let mut ctx = RotationContext::new(false);
        ctx.interpolated_affine = identity_affine();
        // The affine's scale_x/scale_y are baked into total_scale by
        // setupTransform (total_scale = parent_scale * affine.scale_x).
        // For a root deformer, total_scale = affine.scale_x.
        ctx.base.set_total_scale(2.0);

        let src = vec![10.0, 20.0];
        let mut dst = vec![0.0f32; 2];

        rotation_transform_points(&ctx, &src, &mut dst, 1, 0, 2);

        assert!((dst[0] - 20.0).abs() < 1e-5);
        assert!((dst[1] - 40.0).abs() < 1e-5);
    }

    #[test]
    fn test_rotation_transform_reflect_x() {
        let mut ctx = RotationContext::new(false);
        ctx.interpolated_affine = AffineEnt {
            reflect_x: true,
            scale_x: 1.0,
            ..identity_affine()
        };
        ctx.base.set_total_scale(1.0);

        let src = vec![10.0, 20.0];
        let mut dst = vec![0.0f32; 2];

        rotation_transform_points(&ctx, &src, &mut dst, 1, 0, 2);

        assert!((dst[0] - (-10.0)).abs() < 1e-5);
        assert!((dst[1] - 20.0).abs() < 1e-5);
    }

    #[test]
    fn test_tri_lerp_corners() {
        let r0 = tri_lerp(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 0.0, 0.0, 0.0);
        assert!((r0 - 0.0).abs() < 1e-6);

        let r1 = tri_lerp(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 1.0, 1.0, 1.0);
        assert!((r1 - 7.0).abs() < 1e-6);
    }

    #[test]
    fn test_tri_lerp_midpoint() {
        let r = tri_lerp(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 0.5, 0.5, 0.5);
        // Each weight = 0.5*0.5*0.5 = 0.125. Sum of all 8 corners = 0+1+2+3+4+5+6+7 = 28.
        // Result = 28 * 0.125 = 3.5
        assert!((r - 3.5).abs() < 1e-6, "got {}", r);
    }

    #[test]
    fn test_get_angle_not_abs_opposite() {
        let angle = get_angle_not_abs((1.0, 0.0), (-1.0, 0.0));
        assert!((angle - PI).abs() < 1e-5 || (angle + PI).abs() < 1e-5,
            "got {}", angle);
    }
}
