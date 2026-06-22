//! Moc2Model runtime — per-frame deformation pipeline.
//!
//! Phase 4: wraps `Moc2Data` with mutable runtime state and implements
//! the full update pipeline: parameter change detection → deformer
//! interpolation → deformer chain transform → drawable vertex transform
//! → render order sort.

use std::sync::Arc;

use crate::moc2::deformer::{
    self, build_deformer_tree, deformer_get_type, deformer_need_transform,
    drawable_setup_interpolate, rotation_transform_points, warp_setup_interpolate,
    warp_transform_points, WarpContext, RotationContext, TYPE_WARP, TYPE_ROTATION,
};
use crate::moc2::pivot::{ParamPivotState, PivotContext, PIVOT_TABLE_SIZE};
use crate::moc2::types::*;

/// Per-drawable runtime output after a call to [`Moc2Model::update`].
#[derive(Debug, Clone)]
pub struct DrawableState {
    pub interpolated_vertices: Vec<f32>,
    pub transformed_vertices: Vec<f32>,
    pub opacity: f32,
    pub draw_order: i32,
    pub available: bool,
}

impl DrawableState {
    fn new(vertex_count: usize) -> Self {
        let len = vertex_count * 2;
        Self {
            interpolated_vertices: vec![0.0f32; len],
            transformed_vertices: vec![0.0f32; len],
            opacity: 1.0,
            draw_order: 0,
            available: true,
        }
    }
}

/// Full runtime model — one instance per loaded model.
pub struct Moc2Model {
    data: Arc<Moc2Data>,
    param_values: Vec<f32>,
    param_prev_values: Vec<f32>,
    param_updated: Vec<bool>,
    param_ids: Vec<Id>,
    init_version: i32,
    setup_required: bool,
    pivot_states: Vec<ParamPivotState>,
    deformer_parents: Vec<Option<usize>>,
    deformer_order: Vec<usize>,
    warp_states: Vec<Option<WarpContext>>,
    rotation_states: Vec<Option<RotationContext>>,
    drawable_states: Vec<DrawableState>,
    tmp_indices: [u16; PIVOT_TABLE_SIZE],
    tmp_t: [f32; PIVOT_TABLE_SIZE],
    render_order: Vec<usize>,
}

impl Moc2Model {
    pub fn new(data: Arc<Moc2Data>) -> Self {
        let param_ids: Vec<Id> = data.param_defs.iter().map(|p| p.id.clone()).collect();
        let param_values: Vec<f32> =
            data.param_defs.iter().map(|p| p.default_value).collect();

        let (deformer_parents, deformer_order) = build_deformer_tree(&data.deformers);

        let warp_states: Vec<Option<WarpContext>> = data
            .deformers
            .iter()
            .map(|def| match &def.kind {
                DeformerKind::Warp { row, col, .. } => {
                    let need = deformer_need_transform(&def.target_id);
                    let grid_size = ((row + 1) * (col + 1)) as usize;
                    Some(WarpContext::new(need, grid_size))
                }
                DeformerKind::Rotation { .. } => None,
            })
            .collect();

        let rotation_states: Vec<Option<RotationContext>> = data
            .deformers
            .iter()
            .map(|def| match &def.kind {
                DeformerKind::Rotation { .. } => {
                    let need = deformer_need_transform(&def.target_id);
                    Some(RotationContext::new(need))
                }
                DeformerKind::Warp { .. } => None,
            })
            .collect();

        let drawable_states: Vec<DrawableState> = data
            .drawables
            .iter()
            .map(|d| DrawableState::new(d.vertex_count as usize))
            .collect();

        let pivot_states = vec![ParamPivotState::default(); data.param_pivots.len()];

        Self {
            param_prev_values: param_values.clone(),
            param_values,
            param_updated: vec![false; data.param_defs.len()],
            param_ids,
            data,
            init_version: 1,
            setup_required: true,
            pivot_states,
            deformer_parents,
            deformer_order,
            warp_states,
            rotation_states,
            drawable_states,
            tmp_indices: [0u16; PIVOT_TABLE_SIZE],
            tmp_t: [0.0f32; PIVOT_TABLE_SIZE],
            render_order: Vec::with_capacity(256),
        }
    }

    // ── parameter accessors ──

    pub fn set_param_value_by_index(&mut self, index: usize, value: f32) {
        if index >= self.param_values.len() {
            return;
        }
        self.param_values[index] = value.clamp(
            self.data.param_defs[index].min_value,
            self.data.param_defs[index].max_value,
        );
    }

    pub fn set_param_value(&mut self, param_id: &Id, value: f32) {
        if let Some(idx) = self.param_ids.iter().position(|id| id == param_id) {
            self.set_param_value_by_index(idx, value);
        }
    }

    pub fn param_value(&self, index: usize) -> f32 {
        self.param_values.get(index).copied().unwrap_or(0.0)
    }

    pub fn param_values(&self) -> &[f32] {
        &self.param_values
    }

    pub fn param_count(&self) -> usize {
        self.param_values.len()
    }

    pub fn data(&self) -> &Arc<Moc2Data> {
        &self.data
    }

    pub fn drawable_data(&self) -> &[DrawableState] {
        &self.drawable_states
    }

    pub fn render_order(&self) -> &[usize] {
        &self.render_order
    }

    // ── main pipeline ──

    pub fn update(&mut self) {
        let version = self.init_version;
        self.init_version = version.wrapping_add(1);
        let setup = self.setup_required;
        self.setup_required = false;

        self.detect_param_changes(setup);

        // Phase break: extract PivotContext references first,
        // then all subsequent work is done inline (not via &mut self methods).
        let (param_values, param_updated, param_ids, init_version) = {
            (
                &self.param_values[..] as &[f32],
                &self.param_updated[..] as &[bool],
                &self.param_ids[..] as &[Id],
                self.init_version,
            )
        };
        let ctx = PivotContext {
            param_values,
            param_updated,
            param_ids,
            init_version,
            setup_required: setup,
        };

        let order = self.deformer_order.clone();

        // ── Pass 1: setupInterpolate for all deformers ──
        for &def_idx in &order {
            let deformer = &self.data.deformers[def_idx];
            match deformer_get_type(deformer) {
                TYPE_WARP => {
                    let Some(warp) = self.warp_states[def_idx].as_mut() else {
                        continue;
                    };
                    warp_setup_interpolate(
                        deformer,
                        warp,
                        &self.data.param_pivots,
                        &mut self.pivot_states,
                        &ctx,
                        &mut self.tmp_indices,
                        &mut self.tmp_t,
                    );
                }
                TYPE_ROTATION => {
                    let Some(rot) = self.rotation_states[def_idx].as_mut() else {
                        continue;
                    };
                    deformer::rotation_setup_interpolate(
                        deformer,
                        rot,
                        &self.data.param_pivots,
                        &mut self.pivot_states,
                        &ctx,
                        &mut self.tmp_indices,
                        &mut self.tmp_t,
                        &self.data.affines,
                    );
                }
                _ => {}
            }
        }

        // ── Pass 2: setupTransform for all deformers ──
        for &def_idx in &order {
            let deformer = &self.data.deformers[def_idx];
            let need_transform = deformer_need_transform(&deformer.target_id);
            let parent = self.deformer_parents[def_idx];

            match deformer_get_type(deformer) {
                TYPE_WARP => {
                    if !need_transform {
                        continue;
                    }
                    let Some(p) = parent else { continue };

                    // Read interpolated data first (shared ref).
                    let (num_points, src_points) = {
                        let w = self.warp_states[def_idx].as_ref().unwrap();
                        (
                            (w.interpolated_points.len() / 2) as i32,
                            w.interpolated_points.clone(),
                        )
                    };

                    // Mutable borrow this warp state via raw pointer (to coexist
                    // with &self.warp_states passed to apply_deformer_pts).
                    // SAFETY: parent idx != def_idx in a valid deformer tree,
                    // so apply_deformer_pts never reads the element at def_idx.
                    let warp_ptr: *mut Option<WarpContext> =
                        &mut self.warp_states[def_idx];
                    let warp = unsafe { &mut *warp_ptr };
                    if let Some(ref mut w) = warp {
                        if let Some(tf) = w.transformed_points.as_mut() {
                            apply_deformer_pts(
                                &self.data.deformers,
                                &self.warp_states,
                                &self.rotation_states,
                                p,
                                &src_points,
                                tf,
                                num_points,
                                0,
                                2,
                            );
                        }
                    }
                }
                TYPE_ROTATION => {
                    if !need_transform {
                        let rot_ptr: *mut Option<RotationContext> =
                            &mut self.rotation_states[def_idx];
                        let rot = unsafe { &mut *rot_ptr };
                        if let Some(r) = rot {
                            r.transformed_affine = Some(r.interpolated_affine);
                        }
                        continue;
                    }
                    let Some(p) = parent else {
                        let rot_ptr: *mut Option<RotationContext> =
                            &mut self.rotation_states[def_idx];
                        let rot = unsafe { &mut *rot_ptr };
                        if let Some(r) = rot {
                            r.transformed_affine = Some(r.interpolated_affine);
                        }
                        continue;
                    };

                    // Read interpolated affine data first (shared ref).
                    let (ox, oy, rd, rx, ry, sx, sy) = {
                        let r = self.rotation_states[def_idx].as_ref().unwrap();
                        (
                            r.interpolated_affine.origin_x,
                            r.interpolated_affine.origin_y,
                            r.interpolated_affine.rotation_deg,
                            r.interpolated_affine.reflect_x,
                            r.interpolated_affine.reflect_y,
                            r.interpolated_affine.scale_x,
                            r.interpolated_affine.scale_y,
                        )
                    };

                    // Compute parent rotation contribution (uses shared refs only).
                    let src_origin = [ox, oy];
                    let src_dir = [1.0f32, 0.0f32];
                    let mut ret_dir = [0.0f32; 2];

                    get_direction_inline(
                        p,
                        &src_origin,
                        &src_dir,
                        &mut ret_dir,
                        &self.data.deformers,
                        &self.warp_states,
                        &self.rotation_states,
                    );

                    let angle = deformer::get_angle_not_abs(
                        (src_dir[0], src_dir[1]),
                        (ret_dir[0], ret_dir[1]),
                    );

                    // Now mutate transformed_affine via raw pointer.
                    let rot_ptr: *mut Option<RotationContext> =
                        &mut self.rotation_states[def_idx];
                    let rot = unsafe { &mut *rot_ptr };
                    if let Some(r) = rot {
                        let mut out = r.interpolated_affine;
                        out.rotation_deg = rd + angle * deformer::RAD_TO_DEG;
                        out.reflect_x = rx;
                        out.reflect_y = ry;
                        out.scale_x = sx;
                        out.scale_y = sy;
                        out.origin_x = ox;
                        out.origin_y = oy;
                        r.transformed_affine = Some(out);
                    }
                }
                _ => {}
            }
        }

        // ── Pass 3: Drawable processing ──
        self.render_order.clear();
        for draw_idx in 0..self.data.drawables.len() {
            let drawable = &self.data.drawables[draw_idx];
            let ds = &mut self.drawable_states[draw_idx];

            let outside = drawable_setup_interpolate(
                drawable,
                &mut self.pivot_states,
                &ctx,
                &self.data.param_pivots,
                &mut self.tmp_indices,
                &mut self.tmp_t,
                &mut ds.interpolated_vertices,
                &mut ds.draw_order,
                &mut ds.opacity,
            );

            match resolve_drawable_deformer(&self.data.deformers, drawable) {
                None => {
                    ds.transformed_vertices
                        .copy_from_slice(&ds.interpolated_vertices);
                    ds.available = !outside;
                }
                Some(target_def) => {
                    let vert_count = drawable.vertex_count;
                    let chain = build_deformer_chain(target_def, &self.deformer_parents);

                    if let Some(&first) = chain.first() {
                        let src = ds.interpolated_vertices.clone();
                        apply_deformer_pts(
                            &self.data.deformers,
                            &self.warp_states,
                            &self.rotation_states,
                            first,
                            &src,
                            &mut ds.transformed_vertices,
                            vert_count,
                            0,
                            2,
                        );

                        for &next_def in chain.iter().skip(1) {
                            let tmp: Vec<f32> =
                                ds.transformed_vertices[..(vert_count as usize * 2)].to_vec();
                            apply_deformer_pts(
                                &self.data.deformers,
                                &self.warp_states,
                                &self.rotation_states,
                                next_def,
                                &tmp,
                                &mut ds.transformed_vertices,
                                vert_count,
                                0,
                                2,
                            );
                        }
                    } else {
                        ds.transformed_vertices
                            .copy_from_slice(&ds.interpolated_vertices);
                    }

                    ds.available = !outside;
                }
            }

            self.render_order.push(draw_idx);
        }

        self.render_order.sort_by(|&a, &b| {
            self.drawable_states[a]
                .draw_order
                .cmp(&self.drawable_states[b].draw_order)
        });
    }

    fn detect_param_changes(&mut self, setup: bool) {
        for i in 0..self.param_values.len() {
            let changed = setup || self.param_values[i] != self.param_prev_values[i];
            self.param_updated[i] = changed;
            self.param_prev_values[i] = self.param_values[i];
        }
    }
}

// ── free helpers ──

fn apply_deformer_pts(
    deformers: &[Deformer],
    warp_states: &[Option<WarpContext>],
    rotation_states: &[Option<RotationContext>],
    def_idx: usize,
    src: &[f32],
    dst: &mut [f32],
    num: i32,
    off: i32,
    step: i32,
) {
    let deformer = &deformers[def_idx];
    match deformer_get_type(deformer) {
        TYPE_WARP => {
            if let Some(ref warp) = warp_states[def_idx] {
                warp_transform_points(deformer, warp, src, dst, num, off, step);
                return;
            }
        }
        TYPE_ROTATION => {
            if let Some(ref rot) = rotation_states[def_idx] {
                rotation_transform_points(rot, src, dst, num, off, step);
                return;
            }
        }
        _ => {}
    }
    if src.as_ptr() != dst.as_ptr() {
        let copy_len = (num as usize)
            .saturating_mul(step as usize)
            .min(src.len())
            .min(dst.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);
    }
}

fn resolve_drawable_deformer(deformers: &[Deformer], drawable: &Drawable) -> Option<usize> {
    if deformer_need_transform(&drawable.target_id) {
        deformers.iter().position(|d| d.id == drawable.target_id)
    } else {
        None
    }
}

fn build_deformer_chain(target_def: usize, parents: &[Option<usize>]) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut current = Some(target_def);
    while let Some(idx) = current {
        chain.push(idx);
        current = parents[idx];
    }
    chain
}

/// Inline version of `get_direction_on_dst` that takes explicit slices
/// instead of a closure (avoids borrow-checker conflicts).
fn get_direction_inline(
    parent_idx: usize,
    src_origin: &[f32; 2],
    src_dir: &[f32; 2],
    ret_dir: &mut [f32; 2],
    deformers: &[Deformer],
    warp_states: &[Option<WarpContext>],
    rotation_states: &[Option<RotationContext>],
) {
    let mut origin_dst = *src_origin;
    apply_deformer_pts(
        deformers,
        warp_states,
        rotation_states,
        parent_idx,
        src_origin,
        &mut origin_dst,
        1,
        0,
        2,
    );

    let num_steps = 10;
    let mut step = 1.0f32;

    for _ in 0..num_steps {
        let trial_src = [
            src_origin[0] + step * src_dir[0],
            src_origin[1] + step * src_dir[1],
        ];
        let mut trial_dst = [0.0f32; 2];
        apply_deformer_pts(
            deformers,
            warp_states,
            rotation_states,
            parent_idx,
            &trial_src,
            &mut trial_dst,
            1,
            0,
            2,
        );
        trial_dst[0] -= origin_dst[0];
        trial_dst[1] -= origin_dst[1];

        if trial_dst[0] != 0.0 || trial_dst[1] != 0.0 {
            ret_dir[0] = trial_dst[0];
            ret_dir[1] = trial_dst[1];
            return;
        }

        let trial_src2 = [
            src_origin[0] - step * src_dir[0],
            src_origin[1] - step * src_dir[1],
        ];
        let mut trial_dst2 = [0.0f32; 2];
        apply_deformer_pts(
            deformers,
            warp_states,
            rotation_states,
            parent_idx,
            &trial_src2,
            &mut trial_dst2,
            1,
            0,
            2,
        );
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
