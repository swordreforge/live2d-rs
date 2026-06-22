#[test]
fn probe_deformer_tree_trace() {
    use std::collections::HashMap;
    
    let data = std::fs::read("/home/swordreforge/Downloads/live2d-v2-main/resources/Epsilon/Epsilon.moc")
        .expect("read file");
    let parsed = live2d_core::moc2::parse_moc2(&data).expect("parse OK");
    
    // Build deformer tree manually
    let mut parent_of: Vec<Option<usize>> = vec![None; parsed.deformers.len()];
    let mut id_to_idx: HashMap<&str, usize> = HashMap::new();
    for (i, d) in parsed.deformers.iter().enumerate() {
        id_to_idx.insert(d.id.as_ref(), i);
    }
    for (i, d) in parsed.deformers.iter().enumerate() {
        let is_base = d.target_id.is_empty()
            || d.target_id.as_ref() == "DST_BASE"
            || d.target_id.as_ref() == "BASE";
        if !is_base {
            if let Some(&p) = id_to_idx.get(d.target_id.as_ref()) {
                parent_of[i] = Some(p);
            }
        }
    }
    
    // Find drawable chains
    println!("=== DRAWABLE -> FULL CHAIN ===");
    'next_drawable: for (di, d) in parsed.drawables.iter().enumerate().take(66) {
        let is_base = d.target_id.is_empty()
            || d.target_id.as_ref() == "DST_BASE"
            || d.target_id.as_ref() == "BASE";
        
        let chain = if !is_base {
            if let Some(&tgt) = id_to_idx.get(d.target_id.as_ref()) {
                let mut c = vec![tgt];
                let mut cur = tgt;
                while let Some(p) = parent_of[cur] {
                    c.push(p);
                    cur = p;
                }
                c
            } else {
                vec![] // target deformer not found
            }
        } else {
            vec![]
        };
        
        if chain.is_empty() {
            println!("  drawable[{di}]: id={:?} NO_CHAIN (no deformer needed)", d.id);
            continue;
        }
        
        // Show chain with grid bounds
        print!("  drawable[{di}]: id={:?} verts={} chain:", d.id, d.vertex_count);
        for &ci in &chain {
            let def = &parsed.deformers[ci];
            match &def.kind {
                live2d_core::moc2::DeformerKind::Warp { col, row, pivot_points } => {
                    let bounds = if let Some(first) = pivot_points.first() {
                        let xs: Vec<f32> = first.iter().step_by(2).copied().collect();
                        let ys: Vec<f32> = first.iter().skip(1).step_by(2).copied().collect();
                        let (min_x, max_x) = xs.iter().fold((f32::MAX, f32::MIN), |(mn, mx), &x| (mn.min(x), mx.max(x)));
                        let (min_y, max_y) = ys.iter().fold((f32::MAX, f32::MIN), |(mn, mx), &y| (mn.min(y), mx.max(y)));
                        if min_x >= 0.0 && max_x <= 1.0 && min_y >= 0.0 && max_y <= 1.0 {
                            format!("[NORM {:.2}-{:.2} {:.2}-{:.2}]", min_x, max_x, min_y, max_y)
                        } else {
                            format!("[PIXEL {:.1}-{:.1} {:.1}-{:.1}]", min_x, max_x, min_y, max_y)
                        }
                    } else { "[empty]".into() };
                    print!(" [{ci}]Warp{col}x{row}{bounds}");
                },
                live2d_core::moc2::DeformerKind::Rotation { .. } => {
                    print!(" [{ci}]Rot");
                },
            }
        }
        println!();
    }
}
