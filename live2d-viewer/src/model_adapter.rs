//! Adapter layer that bridges MOC2 (pure Rust) and MOC3 (Core FFI) models
//! into a unified interface for the viewer.
//!
//! The renderer and application code work with `LoadedModelVariant`, which
//! dispatches to the appropriate concrete model type.

use std::sync::Arc;

use live2d_core::Model;
use live2d_core::moc2::{Moc2Data, Moc2Model, ColorComposition, DeformerKind};
use live2d_core::canvas::CanvasInfo;

// ---------------------------------------------------------------------------
// Per-frame drawable data — collected once from either model type
// ---------------------------------------------------------------------------

/// Flat per-frame drawable data that the renderer consumes directly.
/// This avoids lifetime tangles between Core's borrowed pointers and our
/// owned MOC2 storage.
pub struct FrameDrawables {
    pub n: usize,
    pub render_orders: Vec<i32>,
    pub tex_indices: Vec<i32>,
    pub opacities: Vec<f32>,
    pub vert_counts: Vec<i32>,
    /// Per-drawable: pointer to the first float of (x,y) pairs.
    pub vert_positions: Vec<*const f32>,
    /// Per-drawable: pointer to the first float of (u,v) pairs.
    pub vert_uvs: Vec<*const f32>,
    pub idx_counts: Vec<i32>,
    /// Per-drawable: pointer to the first u16 of the index array.
    pub indices: Vec<*const u16>,
    /// (r,g,b,a) — α is the clip-mask / offscreen alpha from Core.
    pub mul_colors: Vec<[f32; 4]>,
    pub scr_colors: Vec<[f32; 4]>,
    pub blend_modes: Vec<i32>,
    pub mask_counts: Vec<i32>,
    /// Per-drawable: pointer to i32 mask-indices (length = mask_counts[i]).
    pub masks: Vec<*const i32>,
    /// csmFlags packed as u8.
    pub constant_flags: Vec<u8>,
    pub dynamic_flags: Vec<u8>,
    /// Parent part index per drawable (for debug overlay, unused in rendering).
    pub parent_parts: Vec<i32>,

    // -- owned backing storage (MOC2 path fills these, Core path leaves empty) --
    pub(crate) _backing_pos: Vec<Vec<f32>>,
    pub(crate) _backing_uv: Vec<Vec<f32>>,
    pub(crate) _backing_idx: Vec<Vec<u16>>,
    pub(crate) _backing_masks: Vec<Vec<i32>>,
}

impl FrameDrawables {
    pub fn empty() -> Self {
        Self {
            n: 0,
            render_orders: Vec::new(),
            tex_indices: Vec::new(),
            opacities: Vec::new(),
            vert_counts: Vec::new(),
            vert_positions: Vec::new(),
            vert_uvs: Vec::new(),
            idx_counts: Vec::new(),
            indices: Vec::new(),
            mul_colors: Vec::new(),
            scr_colors: Vec::new(),
            blend_modes: Vec::new(),
            mask_counts: Vec::new(),
            masks: Vec::new(),
            constant_flags: Vec::new(),
            dynamic_flags: Vec::new(),
            parent_parts: Vec::new(),
            _backing_pos: Vec::new(),
            _backing_uv: Vec::new(),
            _backing_idx: Vec::new(),
            _backing_masks: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Unified model variant
// ---------------------------------------------------------------------------

pub enum LoadedModelVariant {
    Core(Model<'static>),
    V2(Box<Moc2ModelAdapter>),
}

// SAFETY: Moc2ModelAdapter contains only Send + Sync types internally.
// The raw pointers in FrameDrawables are transient per-frame and never sent across threads.
unsafe impl Send for LoadedModelVariant {}
unsafe impl Sync for LoadedModelVariant {}

#[allow(dead_code)]
impl LoadedModelVariant {
    /// Run the per-frame update.
    /// For Core models this also resets dynamic flags (csmIsVisible etc.)
    pub fn update(&mut self) {
        match self {
            Self::Core(m) => {
                m.reset_dynamic_flags();
                m.update();
            }
            Self::V2(a) => a.model.update(),
        }
    }

    pub fn canvas_info(&self) -> CanvasInfo {
        match self {
            Self::Core(m) => m.canvas_info(),
            Self::V2(a) => a.canvas_info(),
        }
    }

    // ── Parameters ──

    pub fn param_count(&self) -> usize {
        match self {
            Self::Core(m) => m.parameters().len(),
            Self::V2(a) => a.model.param_count(),
        }
    }

    pub fn param_ids(&self) -> Vec<String> {
        match self {
            Self::Core(m) => m.parameters().ids().into_iter()
                .map(|c| c.to_string_lossy().into_owned())
                .collect(),
            Self::V2(a) => a.data.param_defs.iter()
                .map(|p| p.id.to_string())
                .collect(),
        }
    }

    pub fn param_values(&self) -> Vec<f32> {
        match self {
            Self::Core(m) => m.parameters().values().to_vec(),
            Self::V2(a) => a.model.param_values().to_vec(),
        }
    }

    pub fn param_default_values(&self) -> Vec<f32> {
        match self {
            Self::Core(m) => m.parameters().default_values().to_vec(),
            Self::V2(a) => a.data.param_defs.iter().map(|p| p.default_value).collect(),
        }
    }

    pub fn param_minimum_values(&self) -> Vec<f32> {
        match self {
            Self::Core(m) => m.parameters().minimum_values().to_vec(),
            Self::V2(a) => a.data.param_defs.iter().map(|p| p.min_value).collect(),
        }
    }

    pub fn param_maximum_values(&self) -> Vec<f32> {
        match self {
            Self::Core(m) => m.parameters().maximum_values().to_vec(),
            Self::V2(a) => a.data.param_defs.iter().map(|p| p.max_value).collect(),
        }
    }

    /// Write parameter values back to the model.
    /// `values` must have length == param_count().
    pub fn set_param_values(&mut self, values: &[f32]) {
        match self {
            Self::Core(m) => {
                let mut params = m.parameters();
                let mut vals = params.values_mut();
                for (i, &v) in values.iter().enumerate() {
                    if i < vals.as_mut_slice().len() {
                        vals.set(i, v);
                    }
                }
            }
            Self::V2(a) => {
                for (i, &v) in values.iter().enumerate() {
                    if i < a.model.param_count() {
                        a.model.set_param_value_by_index(i, v);
                    }
                }
            }
        }
    }

    // ── Parts ──

    pub fn part_ids(&self) -> Vec<String> {
        match self {
            Self::Core(m) => m.parts().ids().into_iter()
                .map(|c| c.to_string_lossy().into_owned())
                .collect(),
            Self::V2(a) => a.data.parts.iter()
                .map(|p| p.id.to_string())
                .collect(),
        }
    }

    pub fn part_count(&self) -> usize {
        match self {
            Self::Core(m) => m.parts().len(),
            Self::V2(a) => a.data.parts.len(),
        }
    }

    pub fn part_opacities(&self) -> Vec<f32> {
        match self {
            Self::Core(m) => m.parts().opacities().to_vec(),
            Self::V2(a) => a.data.parts.iter().map(|p| if p.visible { 1.0 } else { 0.0 }).collect(),
        }
    }

    pub fn part_opacities_mut(&mut self) -> PartOpacitiesMut<'_> {
        match self {
            Self::Core(m) => {
                PartOpacitiesMut::Core(unsafe {
                    let ptr = live2d_core_sys::csmGetPartOpacities(
                        m.as_raw() as *mut _
                    );
                    let len = m.parts().len();
                    std::slice::from_raw_parts_mut(ptr, len)
                })
            }
            Self::V2(a) => {
                // MOC2: opacities are static in parts data — store runtime copy in adapter
                PartOpacitiesMut::V2(&mut a.part_opacities_runtime)
            }
        }
    }

    // ── Drawables (per-frame collection) ──

    /// Collect all drawable data for the current frame.
    /// Must be called AFTER `update()`.
    pub fn collect_drawables(&mut self) -> FrameDrawables {
        match self {
            Self::Core(m) => Self::collect_core_drawables(m),
            Self::V2(a) => a.collect_drawables(),
        }
    }

    fn collect_core_drawables(m: &Model<'static>) -> FrameDrawables {
        let drawables = m.drawables();
        let n = drawables.len();
        if n == 0 {
            return FrameDrawables::empty();
        }

        let render_orders = m.render_orders().to_vec();
        let tex_indices = drawables.texture_indices().to_vec();
        let opacities = drawables.opacities().to_vec();
        let vert_counts = drawables.vertex_counts().to_vec();
        let idx_counts = drawables.index_counts().to_vec();
        let mul_colors: Vec<[f32; 4]> = drawables.multiply_colors().iter()
            .map(|v| [v.X, v.Y, v.Z, v.W])
            .collect();
        let scr_colors: Vec<[f32; 4]> = drawables.screen_colors().iter()
            .map(|v| [v.X, v.Y, v.Z, v.W])
            .collect();
        let blend_modes = drawables.blend_modes().to_vec();
        let mask_counts = drawables.mask_counts().to_vec();
        let constant_flags = drawables.constant_flags().to_vec();
        let dynamic_flags = drawables.dynamic_flags().to_vec();
        let parent_parts = drawables.parent_part_indices().to_vec();

        let vpos_ptrs = drawables.vertex_positions();
        let vuv_ptrs = drawables.vertex_uvs();
        let idx_ptrs = drawables.indices();
        let mask_ptrs = drawables.masks();

        let vert_positions: Vec<*const f32> = vpos_ptrs.iter().map(|&p| p.cast()).collect();
        let vert_uvs: Vec<*const f32> = vuv_ptrs.iter().map(|&p| p.cast()).collect();
        let indices: Vec<*const u16> = idx_ptrs.to_vec();
        let masks: Vec<*const i32> = mask_ptrs.to_vec();

        FrameDrawables {
            n,
            render_orders,
            tex_indices,
            opacities,
            vert_counts,
            vert_positions,
            vert_uvs,
            idx_counts,
            indices,
            mul_colors,
            scr_colors,
            blend_modes,
            mask_counts,
            masks,
            constant_flags,
            dynamic_flags,
            parent_parts,
            _backing_pos: Vec::new(),
            _backing_uv: Vec::new(),
            _backing_idx: Vec::new(),
            _backing_masks: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MOC2 adapter
// ---------------------------------------------------------------------------

/// Wraps `Moc2Model` + static `Moc2Data` and presents the same data the
/// renderer expects.
pub struct Moc2ModelAdapter {
    pub model: Moc2Model,
    pub data: Arc<Moc2Data>,
    /// MOC2 parts have static visibility. We maintain a runtime copy so the
    /// pose/motion system can write part opacities (like Core does internally).
    pub part_opacities_runtime: Vec<f32>,
}

impl Moc2ModelAdapter {
    pub fn new(model: Moc2Model, data: Arc<Moc2Data>) -> Self {
        let part_opacities_runtime: Vec<f32> = data.parts.iter()
            .map(|p| if p.visible { 1.0 } else { 0.0 })
            .collect();
        Self { model, data, part_opacities_runtime }
    }

    pub fn canvas_info(&self) -> CanvasInfo {
        let w = self.data.canvas_width as f32;
        let h = self.data.canvas_height as f32;
        // MOC2: pixels_per_unit is usually derived from canvas size.
        // 1620 canvas → 810 logical → ppu = 2.0. Default to 1.0 for safety.
        let ppu = 1.0;
        CanvasInfo {
            size_in_pixels: live2d_core_sys::csmVector2 { X: w, Y: h },
            origin_in_pixels: live2d_core_sys::csmVector2 { X: 0.0, Y: 0.0 },
            pixels_per_unit: ppu,
        }
    }

    fn collect_drawables(&mut self) -> FrameDrawables {
        let drawables = &self.data.drawables;
        let n = drawables.len();
        if n == 0 {
            return FrameDrawables::empty();
        }

        let drawable_states = self.model.drawable_data();

        // ── DEBUG: dump deformer tree + coordinates once ──
        use std::sync::atomic::{AtomicBool, Ordering};
        static DUMPED: AtomicBool = AtomicBool::new(false);
        if !DUMPED.swap(true, Ordering::Relaxed) {
            eprintln!("=== DEFORMER TREE ===");
            let parents = self.model.deformer_parents();
            let order = self.model.deformer_order();
            for &def_idx in order.iter() {
                let def = &self.data.deformers[def_idx];
                let kind = match &def.kind {
                    DeformerKind::Warp { row, col, .. } =>
                        format!("Warp({}x{})", row, col),
                    DeformerKind::Rotation { .. } =>
                        "Rotation".to_string(),
                };
                let parent_info = match parents[def_idx] {
                    Some(p) => format!("parent=#{}", p),
                    None => "root".to_string(),
                };
                eprintln!("  def#{} id={:?} {} target={:?} {}", 
                    def_idx, def.id, kind, def.target_id, parent_info);
            }
            eprintln!("=== DRAWABLE DEFORMER MAP ===");
            for (i, d) in drawables.iter().enumerate() {
                let is_base = d.target_id.is_empty() || *d.target_id == *"DST_BASE" || *d.target_id == *"BASE";
                let target = if is_base {
                    "none(base)".to_string()
                } else {
                    let def_idx = self.data.deformers.iter().position(|def| def.id == d.target_id);
                    match def_idx {
                        Some(idx) => format!("deformer#{}", idx),
                        None => "MISSING!".to_string(),
                    }
                };
                eprintln!("  draw#{} id={:?} -> {}", i, d.id, target);
            }
            // Dump rotation deformer origins (key for hand positioning)
            eprintln!("=== ROTATION ORIGINS (hand chain) ===");
            let hand_rot_ids = ["B_HAND.08", "B_HAND.17", "B_HAND.18", "B_CLOTHES.04", "B_CLOTHES.07", 
                "B_CLOTHES.38", "B_CLOTHES.39", "B_CLOTHES.40", "B_CLOTHES.41",
                "B_CLOTHES.31", "B_CLOTHES.32", "B_CLOTHES.33"];
            for (i, rot) in self.model.rotation_states().iter().enumerate() {
                if let Some(r) = rot {
                    let name = &self.data.deformers[i].id;
                    if hand_rot_ids.contains(&name.as_ref()) {
                        let ia = r.interpolated_affine();
                        let ta = r.transformed_affine_ref();
                        eprintln!("  rot#{} id={:?} interp=({:.1},{:.1}) rot={:.1} scale=({:.3},{:.3})", 
                            i, name, ia.0, ia.1, ia.2, ia.3, ia.4);
                        if let Some(ta) = ta {
                            eprintln!("         trans=({:.1},{:.1}) rot={:.1} scale=({:.3},{:.3})", 
                                ta.0, ta.1, ta.2, ta.3, ta.4);
                        }
                    }
                }
            }
            // Dump bounding boxes of all drawables
            eprintln!("=== DRAWABLE BOUNDING BOXES ===");
            for (i, d) in drawables.iter().enumerate() {
                let ds = if i < drawable_states.len() { &drawable_states[i] } else { continue };
                let vc = d.vertex_count as usize;
                if ds.transformed_vertices.len() >= vc * 2 {
                    let mut min_x = f32::MAX;
                    let mut min_y = f32::MAX;
                    let mut max_x = f32::MIN;
                    let mut max_y = f32::MIN;
                    for j in 0..vc {
                        let x = ds.transformed_vertices[j*2];
                        let y = ds.transformed_vertices[j*2+1];
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                    eprintln!("  draw#{} id={:?} bbox=[{:.0},{:.0}]-[{:.0},{:.0}] opacity={:.3} base_op={:.3} available={}", 
                        i, d.id, min_x, min_y, max_x, max_y, ds.opacity, ds.base_opacity, ds.available);
                }
            }
        }

        let mut fd = FrameDrawables {
            n,
            render_orders: Vec::with_capacity(n),
            tex_indices: Vec::with_capacity(n),
            opacities: Vec::with_capacity(n),
            vert_counts: Vec::with_capacity(n),
            vert_positions: Vec::with_capacity(n),
            vert_uvs: Vec::with_capacity(n),
            idx_counts: Vec::with_capacity(n),
            indices: Vec::with_capacity(n),
            mul_colors: Vec::with_capacity(n),
            scr_colors: Vec::with_capacity(n),
            blend_modes: Vec::with_capacity(n),
            mask_counts: Vec::with_capacity(n),
            masks: Vec::with_capacity(n),
            constant_flags: Vec::with_capacity(n),
            dynamic_flags: Vec::with_capacity(n),
            parent_parts: Vec::with_capacity(n),
            _backing_pos: Vec::with_capacity(n),
            _backing_uv: Vec::with_capacity(n),
            _backing_idx: Vec::with_capacity(n),
            _backing_masks: Vec::with_capacity(n),
        };

        // Build render_orders array with actual draw_order values per drawable index
        for ds in drawable_states.iter().take(n) {
            fd.render_orders.push(ds.draw_order);
        }

        for (i, d) in drawables.iter().enumerate() {
            let ds = if i < drawable_states.len() { &drawable_states[i] } else { continue };

            // Parent part index: map drawable back to its owning part
            let part_idx = self.data.parts.iter()
                .position(|p| p.drawable_indices.contains(&i))
                .unwrap_or(0) as i32;
            let part_opacity = if (part_idx as usize) < self.part_opacities_runtime.len() {
                self.part_opacities_runtime[part_idx as usize]
            } else {
                1.0
            };

            // Python reference: opacity = drawable.opacity * partsOpacity * baseOpacity
            fd.tex_indices.push(d.texture_no);
            let final_opacity = ds.opacity * part_opacity * ds.base_opacity;
            fd.opacities.push(final_opacity);
            fd.vert_counts.push(d.vertex_count);
            fd.idx_counts.push(d.index_array.len() as i32);

            // Position data (transformed vertices from runtime)
            let vc = d.vertex_count as usize;
            let pos_data = if ds.transformed_vertices.len() >= vc * 2 {
                ds.transformed_vertices[..vc * 2].to_vec()
            } else {
                vec![0.0f32; vc * 2]
            };
            fd.vert_positions.push(pos_data.as_ptr());
            fd._backing_pos.push(pos_data);

            // UV data (static from Moc2Data)
            let uv_data = if d.uvs.len() >= vc * 2 {
                d.uvs[..vc * 2].to_vec()
            } else {
                vec![0.0f32; vc * 2]
            };
            fd.vert_uvs.push(uv_data.as_ptr());
            fd._backing_uv.push(uv_data);

            // Index data (static from Moc2Data)
            let idx_data = d.index_array.clone();
            fd.indices.push(idx_data.as_ptr());
            fd._backing_idx.push(idx_data);

            // Default multiply/screen colors (MOC2 doesn't have per-vertex colors)
            fd.mul_colors.push([1.0, 1.0, 1.0, 1.0]);
            fd.scr_colors.push([0.0, 0.0, 0.0, 0.0]);

            // Blend mode
            let (blend_mode, _) = match d.color_composition {
                ColorComposition::Normal => (0, "Normal"),
                ColorComposition::Screen => (1, "Screen"),
                ColorComposition::Multiply => (2, "Multiply"),
            };
            fd.blend_modes.push(blend_mode);

            // No masks in MOC2
            fd.mask_counts.push(0);
            fd.masks.push(std::ptr::null());

            // Constant flags: no inverted mask in MOC2
            fd.constant_flags.push(0u8);
            // Dynamic flags: visible if final opacity > 0
            let is_vis = if final_opacity > 0.001 && ds.available { 1u8 } else { 0u8 };
            fd.dynamic_flags.push(is_vis);

            fd.parent_parts.push(part_idx);
        }

        fd
    }
}

// ---------------------------------------------------------------------------
// Part opacities mutable reference
// ---------------------------------------------------------------------------

pub enum PartOpacitiesMut<'a> {
    Core(&'a mut [f32]),
    V2(&'a mut [f32]),
}

impl PartOpacitiesMut<'_> {
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        match self {
            Self::Core(s) => s,
            Self::V2(s) => s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live2d_core::moc2::parse_moc2;

    const MOC2_PATH: &str = "/home/swordreforge/Downloads/live2d-v2-main/test-data/model.moc";

    #[test]
    fn test_adapter_parse_and_collect_drawables() {
        let path = std::path::Path::new(MOC2_PATH);
        if !path.exists() {
            eprintln!("[SKIP] test data not found at {MOC2_PATH}");
            return;
        }

        let data = std::fs::read(path).expect("read model.moc");
        let parsed = parse_moc2(&data).expect("parse MOC2");

        let n_params = parsed.param_defs.len();
        let n_parts = parsed.parts.len();
        let n_deformers = parsed.deformers.len();

        let data = Arc::new(parsed);
        let runtime = Moc2Model::new(data.clone());
        let adapter = Moc2ModelAdapter::new(runtime, data);
        let mut model = LoadedModelVariant::V2(Box::new(adapter));

        let canvas = model.canvas_info();
        assert!(canvas.size_in_pixels.X > 0.0);
        assert!(canvas.size_in_pixels.Y > 0.0);

        model.update();

        // may be 0 for parameter-only models
        let fd = model.collect_drawables();
        eprintln!("MOC2: {} params, {} parts, {} deformers, {} drawables",
            n_params, n_parts, n_deformers, fd.n);

        let mut popac = model.part_opacities_mut();
        let slice = popac.as_mut_slice();
        assert_eq!(slice.len(), n_parts);
        for &v in slice.iter() {
            assert_eq!(v, 1.0, "part opacity should be 1.0 after update");
        }
    }
}
