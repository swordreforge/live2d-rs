//! Integration test: parse a real Cubism 2.1 .moc file.

use std::path::Path;

const TEST_MOC_PATH: &str = "/home/swordreforge/Downloads/live2d-v2-main/test-data/model.moc";

#[test]
fn test_parse_model_moc() {
    let path = Path::new(TEST_MOC_PATH);
    if !path.exists() {
        eprintln!("[SKIP] test data not found at {TEST_MOC_PATH}");
        return;
    }

    let data = std::fs::read(path).expect("read .moc file");
    let model = live2d_core::moc2::parse_moc2(&data).expect("parse MOC2");

    assert!(model.canvas_width > 0, "canvas_width should be positive");
    assert!(model.canvas_height > 0, "canvas_height should be positive");
    assert!(!model.param_defs.is_empty(), "should have at least one param");
    assert!(!model.parts.is_empty(), "should have at least one part");

    println!("=== MOC2 parse OK ===");
    println!("  Canvas: {} x {}", model.canvas_width, model.canvas_height);
    println!("  Params: {}  Parts: {}  Deformers: {}  Drawables: {}",
        model.param_defs.len(), model.parts.len(),
        model.deformers.len(), model.drawables.len());
    println!("  PivotManagers: {}  ParamPivots: {}  AffineEnts: {}",
        model.pivot_managers.len(), model.param_pivots.len(),
        model.affines.len());

    for (i, p) in model.param_defs.iter().enumerate() {
        println!("  Param[{i}]: \"{}\" [{}, {}] default={}",
            p.id, p.min_value, p.max_value, p.default_value);
    }

    for (i, d) in model.deformers.iter().enumerate() {
        let kind = match d.kind {
            live2d_core::moc2::DeformerKind::Warp { col, row, .. } =>
                format!("Warp({}x{})", col, row),
            live2d_core::moc2::DeformerKind::Rotation { ref affine_indices } =>
                format!("Rotation({} affines)", affine_indices.len()),
        };
        println!("  Deformer[{i}]: id=\"{}\" target=\"{}\" kind={kind} pmgr={}",
            d.id, d.target_id, d.pivot_manager_index);
    }

    for (i, d) in model.drawables.iter().enumerate() {
        println!("  Drawable[{i}]: id=\"{}\" tex={} verts={} tris={} order={} clip={:?}",
            d.id, d.texture_no, d.vertex_count,
            d.index_array.len() / 3,
            d.average_draw_order,
            d.clip_id.as_deref());
    }
}
