//! Registry → domain struct resolution (Phase 2).
//!
//! Two-pass approach:
//!   1. Scan every Opaque entry and record its registry-index → domain-index
//!      mapping (`idx_to_*`) so forward references work regardless of order.
//!   2. Walk the entries again and build the final domain structs,
//!      resolving cross-references via the maps built in pass 1.

use std::collections::HashMap;

use moc_parser::{Blob, MocResult, ObjIndex, Registry};

use super::types::*;
use super::resolve_util::*;

/// Post-process a parsed [`Registry`] into a [`Moc2Data`].
pub(crate) fn resolve_registry(registry: &Registry) -> MocResult<Moc2Data> {
    let mut ids: HashMap<ObjIndex, Id> = HashMap::new();

    // Pass 1: collect every domain type's registry-index → Vec-position mapping.
    let mut idx_to_param_def: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_parts: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_deformer: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_drawable: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_pmgr: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_ppivot: HashMap<ObjIndex, usize> = HashMap::new();
    let mut idx_to_affine: HashMap<ObjIndex, usize> = HashMap::new();

    for i in 0..registry.len() {
        let idx = i as ObjIndex;
        match registry.get(idx) {
            Ok(Blob::String(s)) => {
                ids.insert(idx, s.as_ref().into());
            }
            Ok(Blob::Opaque(data)) => {
                let mut pos = 0usize;
                // Count slots (no domain struct built yet)
                match read_tag(data, &mut pos) {
                    131 => { idx_to_param_def.insert(idx, 0); }
                    133 => { idx_to_parts.insert(idx, 0); }
                    65 | 68 => { idx_to_deformer.insert(idx, 0); }
                    66 => { idx_to_pmgr.insert(idx, 0); }
                    67 => { idx_to_ppivot.insert(idx, 0); }
                    69 => { idx_to_affine.insert(idx, 0); }
                    70 => { idx_to_drawable.insert(idx, 0); }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Assign stable positions.
    assign_positions(&mut idx_to_param_def);
    assign_positions(&mut idx_to_parts);
    assign_positions(&mut idx_to_deformer);
    assign_positions(&mut idx_to_drawable);
    assign_positions(&mut idx_to_pmgr);
    assign_positions(&mut idx_to_ppivot);
    assign_positions(&mut idx_to_affine);

    // Helper: empty Id.
    let empty_id: Id = "".into();

    // Pre-allocate vectors.
    let mut param_defs = vec![
        ParamDef { id: empty_id.clone(), min_value: 0.0, max_value: 0.0, default_value: 0.0 };
        idx_to_param_def.len()
    ];
    let mut parts = vec![
        PartsData { id: empty_id.clone(), locked: false, visible: false, deformer_indices: Vec::new(), drawable_indices: Vec::new() };
        idx_to_parts.len()
    ];
    let mut deformers = vec![
        Deformer { id: empty_id.clone(), target_id: empty_id.clone(), pivot_manager_index: usize::MAX, pivot_opacities: Vec::new(), kind: DeformerKind::Warp { col: 0, row: 0, pivot_points: Vec::new() } };
        idx_to_deformer.len()
    ];
    let mut drawables = vec![
        Drawable { id: empty_id.clone(), target_id: empty_id.clone(), pivot_manager_index: usize::MAX, average_draw_order: 0, pivot_draw_orders: Vec::new(), pivot_opacities: Vec::new(), clip_id: None, texture_no: 0, vertex_count: 0, index_array: Vec::new(), pivot_points: Vec::new(), uvs: Vec::new(), color_composition: ColorComposition::Normal, culling: true };
        idx_to_drawable.len()
    ];
    let mut pivot_managers = vec![
        PivotManager { pivot_indices: Vec::new() };
        idx_to_pmgr.len()
    ];
    let mut param_pivots = vec![
        ParamPivot { param_id: empty_id.clone(), pivot_count: 0, pivot_values: Vec::new() };
        idx_to_ppivot.len()
    ];
    let mut affines = vec![
        AffineEnt { origin_x: 0.0, origin_y: 0.0, scale_x: 0.0, scale_y: 0.0, rotation_deg: 0.0, reflect_x: false, reflect_y: false };
        idx_to_affine.len()
    ];

    // Pass 2: build domain structs, resolving cross-references.
    for i in 0..registry.len() {
        let idx = i as ObjIndex;
        if let Ok(Blob::Opaque(data)) = registry.get(idx) {
            let mut pos = 0usize;
            match read_tag(data, &mut pos) {
                131 => {
                    let min_v = read_f32(data, &mut pos);
                    let max_v = read_f32(data, &mut pos);
                    let def_v = read_f32(data, &mut pos);
                    let pid = read_u32(data, &mut pos);
                    let slot = idx_to_param_def[&idx];
                    param_defs[slot] = ParamDef {
                        id: resolve_id(pid, &ids),
                        min_value: min_v,
                        max_value: max_v,
                        default_value: def_v,
                    };
                }
                133 => {
                    let locked = read_u8(data, &mut pos) != 0;
                    let visible = read_u8(data, &mut pos) != 0;
                    let id_idx = read_u32(data, &mut pos);
                    let def_list = read_u32(data, &mut pos);
                    let draw_list = read_u32(data, &mut pos);
                    let slot = idx_to_parts[&idx];
                    parts[slot] = PartsData {
                        id: resolve_id(id_idx, &ids),
                        locked,
                        visible,
                        deformer_indices: resolve_obj_array(def_list, registry, &idx_to_deformer),
                        drawable_indices: resolve_obj_array(draw_list, registry, &idx_to_drawable),
                    };
                }
                65 => {
                    let id_idx = read_u32(data, &mut pos);
                    let target_idx = read_u32(data, &mut pos);
                    let col = read_i32(data, &mut pos);
                    let row = read_i32(data, &mut pos);
                    let pmgr = read_u32(data, &mut pos);
                    let ppts = read_u32(data, &mut pos);
                    let popac = read_u32(data, &mut pos);
                    let slot = idx_to_deformer[&idx];
                    deformers[slot] = Deformer {
                        id: resolve_id(id_idx, &ids),
                        target_id: resolve_id(target_idx, &ids),
                        pivot_manager_index: resolve_index(pmgr, &idx_to_pmgr).unwrap_or(usize::MAX),
                        pivot_opacities: if popac != u32::MAX { resolve_f32_array(popac, registry) } else { Vec::new() },
                        kind: DeformerKind::Warp {
                            col,
                            row,
                            pivot_points: resolve_f32_array_array(ppts, registry),
                        },
                    };
                }
                68 => {
                    let id_idx = read_u32(data, &mut pos);
                    let target_idx = read_u32(data, &mut pos);
                    let pmgr = read_u32(data, &mut pos);
                    let affines_arr = read_u32(data, &mut pos);
                    let popac = read_u32(data, &mut pos);
                    let slot = idx_to_deformer[&idx];
                    deformers[slot] = Deformer {
                        id: resolve_id(id_idx, &ids),
                        target_id: resolve_id(target_idx, &ids),
                        pivot_manager_index: resolve_index(pmgr, &idx_to_pmgr).unwrap_or(usize::MAX),
                        pivot_opacities: if popac != u32::MAX { resolve_f32_array(popac, registry) } else { Vec::new() },
                        kind: DeformerKind::Rotation {
                            affine_indices: resolve_obj_array(affines_arr, registry, &idx_to_affine),
                        },
                    };
                }
                66 => {
                    let pivots_arr = read_u32(data, &mut pos);
                    let slot = idx_to_pmgr[&idx];
                    pivot_managers[slot] = PivotManager {
                        pivot_indices: resolve_obj_array(pivots_arr, registry, &idx_to_ppivot),
                    };
                }
                67 => {
                    let pid = read_u32(data, &mut pos);
                    let count = read_i32(data, &mut pos);
                    let pvals = read_u32(data, &mut pos);
                    let slot = idx_to_ppivot[&idx];
                    param_pivots[slot] = ParamPivot {
                        param_id: resolve_id(pid, &ids),
                        pivot_count: count,
                        pivot_values: resolve_f32_array(pvals, registry),
                    };
                }
                69 => {
                    let ox = read_f32(data, &mut pos);
                    let oy = read_f32(data, &mut pos);
                    let sx = read_f32(data, &mut pos);
                    let sy = read_f32(data, &mut pos);
                    let rot = read_f32(data, &mut pos);
                    let rx = read_u8(data, &mut pos) != 0;
                    let ry = read_u8(data, &mut pos) != 0;
                    let slot = idx_to_affine[&idx];
                    affines[slot] = AffineEnt {
                        origin_x: ox,
                        origin_y: oy,
                        scale_x: sx,
                        scale_y: sy,
                        rotation_deg: rot,
                        reflect_x: rx,
                        reflect_y: ry,
                    };
                }
                70 => {
                    let id_idx = read_u32(data, &mut pos);
                    let target_idx = read_u32(data, &mut pos);
                    let pmgr = read_u32(data, &mut pos);
                    let avg_order = read_i32(data, &mut pos);
                    let porders = read_u32(data, &mut pos);
                    let popac = read_u32(data, &mut pos);
                    let clip = read_u32(data, &mut pos);
                    let tex_no = read_i32(data, &mut pos);
                    let idx_arr = read_u32(data, &mut pos);
                    let ppts = read_u32(data, &mut pos);
                    let uvs_arr = read_u32(data, &mut pos);
                    let _opt_flag = read_i32(data, &mut pos);
                    let color_comp = read_u8(data, &mut pos);
                    let cull = read_u8(data, &mut pos) != 0;
                    let pivot_arrays = resolve_f32_array_array(ppts, registry);
                    let vertex_count = pivot_arrays.first().map(|a| a.len() as i32 / 2).unwrap_or(0);
                    let pivot_points: Vec<f32> = pivot_arrays.into_iter().flatten().collect();
                    let slot = idx_to_drawable[&idx];
                    drawables[slot] = Drawable {
                        id: resolve_id(id_idx, &ids),
                        target_id: resolve_id(target_idx, &ids),
                        pivot_manager_index: resolve_index(pmgr, &idx_to_pmgr).unwrap_or(usize::MAX),
                        average_draw_order: avg_order,
                        pivot_draw_orders: resolve_i32_array(porders, registry),
                        pivot_opacities: resolve_f32_array(popac, registry),
                        clip_id: if clip != u32::MAX { Some(resolve_id(clip, &ids)) } else { None },
                        texture_no: tex_no,
                        vertex_count,
                        index_array: resolve_u16_array(idx_arr, registry),
                        pivot_points,
                        uvs: resolve_f32_array(uvs_arr, registry),
                        color_composition: match color_comp {
                            1 => ColorComposition::Screen,
                            2 => ColorComposition::Multiply,
                            _ => ColorComposition::Normal,
                        },
                        culling: cull,
                    };
                }
                _ => {}
            }
        }
    }

    let mut canvas_width = 0i32;
    let mut canvas_height = 0i32;
    for i in 0..registry.len() {
        if let Ok(Blob::Opaque(data)) = registry.get(i as ObjIndex) {
            let mut pos = 0usize;
            if read_tag(data, &mut pos) == 136 {
                let _pdef_set = read_u32(data, &mut pos);
                let _parts_list = read_u32(data, &mut pos);
                canvas_width = read_i32(data, &mut pos);
                canvas_height = read_i32(data, &mut pos);
                break;
            }
        }
    }

    let mut render_order: Vec<usize> = (0..drawables.len()).collect();
    render_order.sort_by_key(|&i| drawables[i].average_draw_order);

    Ok(Moc2Data {
        canvas_width,
        canvas_height,
        param_defs,
        parts,
        deformers,
        drawables,
        pivot_managers,
        param_pivots,
        affines,
        render_order,
    })
}

/// Assign ascending positions to all entries in a map of discovered indices.
fn assign_positions(map: &mut HashMap<ObjIndex, usize>) {
    for (i, (_, slot)) in map.iter_mut().enumerate() {
        *slot = i;
    }
}
