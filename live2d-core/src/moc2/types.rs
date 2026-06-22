//! MOC2 domain type definitions.
//!
//! These structs represent the parsed Live2D 2.x model data.  They are
//! the output of Phase 1 parsing and the input to Phase 2–3 interpolation.

use std::sync::Arc;

/// A single MOC2 string ID (shared across the model).
pub type Id = Arc<str>;

// ── top-level model ────────────────────────────────────────────────

/// The fully-parsed MOC2 model data — static, `Send + Sync`, shareable
/// across threads.
#[derive(Debug, Clone)]
pub struct Moc2Data {
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub param_defs: Vec<ParamDef>,
    pub parts: Vec<PartsData>,
    pub deformers: Vec<Deformer>,
    pub drawables: Vec<Drawable>,
    pub pivot_managers: Vec<PivotManager>,
    pub param_pivots: Vec<ParamPivot>,
    pub affines: Vec<AffineEnt>,
    /// Index into `drawables[]` in render order (sorted).
    pub render_order: Vec<usize>,
}

// ── parameters ─────────────────────────────────────────────────────

/// Single parameter definition.
#[derive(Debug, Clone)]
pub struct ParamDef {
    pub id: Id,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
}

// ── parts ──────────────────────────────────────────────────────────

/// A parts entry — groups deformers and drawables under one visibility.
#[derive(Debug, Clone)]
pub struct PartsData {
    pub id: Id,
    pub locked: bool,
    pub visible: bool,
    pub deformer_indices: Vec<usize>,
    pub drawable_indices: Vec<usize>,
}

// ── deformers ──────────────────────────────────────────────────────

/// A deformer — either warp (grid) or rotation (affine).
#[derive(Debug, Clone)]
pub struct Deformer {
    pub id: Id,
    pub target_id: Id,
    pub pivot_manager_index: usize,
    pub pivot_opacities: Vec<f32>,
    pub kind: DeformerKind,
}

#[derive(Debug, Clone)]
pub enum DeformerKind {
    Warp {
        col: i32,
        row: i32,
        pivot_points: Vec<f32>,
    },
    Rotation {
        affine_indices: Vec<usize>,
    },
}

/// Single affine keyframe for a RotationDeformer.
#[derive(Debug, Clone, Copy)]
pub struct AffineEnt {
    pub origin_x: f32,
    pub origin_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_deg: f32,
    pub reflect_x: bool,
    pub reflect_y: bool,
}

// ── pivot system ───────────────────────────────────────────────────

/// PivotManager — the multi-parameter interpolation controller.
#[derive(Debug, Clone)]
pub struct PivotManager {
    pub pivot_indices: Vec<usize>,
}

/// Single parameter's pivot table.
#[derive(Debug, Clone)]
pub struct ParamPivot {
    pub param_id: Id,
    pub pivot_count: i32,
    pub pivot_values: Vec<f32>,
}

// ── drawables (mesh) ───────────────────────────────────────────────

/// A drawable mesh.
#[derive(Debug, Clone)]
pub struct Drawable {
    pub id: Id,
    pub target_id: Id,
    pub pivot_manager_index: usize,
    pub average_draw_order: i32,
    pub pivot_draw_orders: Vec<i32>,
    pub pivot_opacities: Vec<f32>,
    /// Index of the clip drawable, if any.
    pub clip_id: Option<Id>,
    pub texture_no: i32,
    pub vertex_count: i32,
    pub index_array: Vec<u16>,
    pub pivot_points: Vec<f32>,
    pub uvs: Vec<f32>,
    pub color_composition: ColorComposition,
    pub culling: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorComposition {
    Normal,
    Screen,
    Multiply,
}

// ── Avatar (sub-component) ─────────────────────────────────────────

/// An Avatar groups drawables and deformers under one id.
#[derive(Debug, Clone)]
pub struct Avatar {
    pub id: Id,
    pub drawable_indices: Vec<usize>,
    pub deformer_indices: Vec<usize>,
}
