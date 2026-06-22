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
        let (kind_str, pivot_len) = match &d.kind {
            live2d_core::moc2::DeformerKind::Warp { col, row, pivot_points } =>
                (format!("Warp({}x{})", col, row), pivot_points.len()),
            live2d_core::moc2::DeformerKind::Rotation { affine_indices } =>
                (format!("Rotation({} affines)", affine_indices.len()), 0),
        };
        println!("  Deformer[{i}]: id=\"{}\" target=\"{}\" kind={kind_str} pivot_len={pivot_len} pmgr={}",
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

#[test]
fn test_moc2_model_runtime() {
    let path = Path::new(TEST_MOC_PATH);
    if !path.exists() {
        eprintln!("[SKIP] test data not found at {TEST_MOC_PATH}");
        return;
    }

    let data = std::fs::read(path).expect("read .moc file");
    let parsed = live2d_core::moc2::parse_moc2(&data).expect("parse MOC2");
    let param_count = parsed.param_defs.len();
    let angle_x_index = parsed.param_defs.iter()
        .position(|p| p.id.as_ref() == "PARAM_ANGLE_X")
        .expect("PARAM_ANGLE_X should exist in model");

    let mut model = live2d_core::moc2::Moc2Model::new(std::sync::Arc::new(parsed));

    // Check initial state
    assert_eq!(model.param_count(), param_count,
        "param_count should match parsed data");

    // Default values should match param definitions
    println!("=== Initial parameter values ===");
    for i in 0..model.param_count() {
        println!("  param[{i}] = {}", model.param_value(i));
    }

    // Run first update
    model.update();

    // Check drawable output
    let drawables = model.drawable_data();
    let render_order = model.render_order();
    println!("=== After first update ===");
    println!("  Drawables: {}  Render order: {:?}", drawables.len(), render_order);

    for (i, d) in drawables.iter().enumerate() {
        println!(
            "  Drawable[{i}]: {} verts, opacity={:.4}, order={}, available={}",
            d.transformed_vertices.len() / 2,
            d.opacity,
            d.draw_order,
            d.available,
        );
        // All vertex values should be finite
        for &v in &d.transformed_vertices {
            assert!(v.is_finite(), "vertex value should be finite");
        }
    }

    // Set parameter value and update
    model.set_param_value_by_index(angle_x_index, 15.0);
    model.update();

    let drawables = model.drawable_data();
    println!("=== After PARAM_ANGLE_X = 15 ===");
    for (i, d) in drawables.iter().enumerate() {
        println!(
            "  Drawable[{i}]: {} verts, opacity={:.4}, order={}",
            d.transformed_vertices.len() / 2,
            d.opacity,
            d.draw_order,
        );
    }

    // Second update with same value — should short-circuit
    model.set_param_value_by_index(angle_x_index, 15.0);
    model.update();
    println!("=== After second update (same value) — should short-circuit ===");

    // Update with different value
    model.set_param_value_by_index(angle_x_index, -30.0);
    model.update();
    println!("=== After PARAM_ANGLE_X = -30 ===");
}
