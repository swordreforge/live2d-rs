use std::sync::Arc;

#[test]
fn probe_drawable_state() {
    let path = "/home/swordreforge/Downloads/live2d-v2-main/resources/Epsilon/Epsilon.moc";
    let data = std::fs::read(path).expect("read file");
    let parsed = live2d_core::moc2::parse_moc2(&data).expect("parse OK");

    let data = Arc::new(parsed);
    let mut runtime = live2d_core::moc2::Moc2Model::new(data.clone());

    // Run update (setup frame — all params at defaults)
    runtime.update();

    let drawable_states = runtime.drawable_data();
    let _render_order = runtime.render_order();

    let nd = data.drawables.len();
    println!("=== Drawable State Probe ===");
    println!("Total drawables: {nd}");
    println!();

    // Count available vs unavailable
    let mut available_count = 0;
    let mut unavailable_count = 0;
    let mut nan_count = 0;

    for (i, ds) in drawable_states.iter().enumerate() {
        let d = &data.drawables[i];
        let vc = d.vertex_count as usize;

        let available = ds.available;
        let opacity = ds.opacity;
        let order = ds.draw_order;

        // Check transformed vertices for issues
        let tv = &ds.transformed_vertices;
        let valid_count = if tv.len() >= vc * 2 { vc * 2 } else { 0 };
        let tex = d.texture_no;
        let mut has_nan = false;
        let mut has_inf = false;
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        for &v in tv.iter().take(valid_count) {
            if v.is_nan() { has_nan = true; }
            if v.is_infinite() { has_inf = true; }
            if v < min_val { min_val = v; }
            if v > max_val { max_val = v; }
        }

        // Check interpolated vertices too
        let iv = &ds.interpolated_vertices;
        let mut i_min = f32::MAX;
        let mut i_max = f32::MIN;
        for &v in iv.iter().take(valid_count) {
            if v < i_min { i_min = v; }
            if v > i_max { i_max = v; }
        }

        if !available { unavailable_count += 1; }
        else { available_count += 1; }
        if has_nan { nan_count += 1; }

        if !available || has_nan || has_inf || valid_count == 0 {
            println!("  [{i}] {:?}: tex={tex} avail={available} opacity={opacity:.4} order={order} verts={vc} valid={valid_count} nan={has_nan} inf={has_inf} tv=[{:.2}..{:.2}] iv=[{:.2}..{:.2}]",
                d.id, min_val, max_val, i_min, i_max);
        }
    }

    println!();
    println!("Available: {available_count}/{nd}");
    println!("Unavailable: {unavailable_count}/{nd}");
    println!("Has NaN: {nan_count}/{nd}");

    let mut tex_set: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for d in data.drawables.iter() {
        tex_set.insert(d.texture_no);
    }
    println!("Texture indices used: {:?}", tex_set.iter().collect::<Vec<_>>());
    println!();

    // Build deformer tree for context
    let mut id_to_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, d) in data.deformers.iter().enumerate() {
        id_to_idx.insert(d.id.as_ref(), i);
    }
    let mut parent_of: Vec<Option<usize>> = vec![None; data.deformers.len()];
    for (i, d) in data.deformers.iter().enumerate() {
        let is_base = d.target_id.is_empty()
            || d.target_id.as_ref() == "DST_BASE"
            || d.target_id.as_ref() == "BASE";
        if !is_base {
            if let Some(&p) = id_to_idx.get(d.target_id.as_ref()) {
                parent_of[i] = Some(p);
            }
        }
    }

    // Show all unavailable drawables with their target deformer chain
    if unavailable_count > 0 {
        println!("=== UNAVAILABLE DRAWABLES ===");
        for (i, ds) in drawable_states.iter().enumerate() {
            if ds.available { continue; }
            let d = &data.drawables[i];
            let target_id = d.target_id.as_ref();
            let target_idx = id_to_idx.get(target_id);
            match target_idx {
                Some(&def_idx) => {
                    let def = &data.deformers[def_idx];
                    let kind_str = match &def.kind {
                        live2d_core::moc2::DeformerKind::Warp { .. } => "Warp",
                        live2d_core::moc2::DeformerKind::Rotation { .. } => "Rotation",
                    };
                    print!("  [{i}] {:?} → deformer[{def_idx}] {kind_str} id={:?} chain: {def_idx}", d.id, def.id);
                    let mut cur = def_idx;
                    while let Some(p) = parent_of[cur] {
                        let pdef = &data.deformers[p];
                        let pk = match &pdef.kind {
                            live2d_core::moc2::DeformerKind::Warp { .. } => "Warp",
                            live2d_core::moc2::DeformerKind::Rotation { .. } => "Rotation",
                        };
                        print!(" ← [{p}]{pk}({:?})", pdef.id);
                        cur = p;
                    }
                    println!();
                },
                None => {
                    println!("  [{i}] {:?} → target_id={target_id:?} NOT FOUND in deformers", d.id);
                }
            }
        }
    }

    // Check: do any drawables target a rotation deformer directly?
    println!();
    println!("=== DRAWABLE -> FIRST DEFORMER TYPE ===");
    for (i, d) in data.drawables.iter().enumerate() {
        let is_base = d.target_id.is_empty()
            || d.target_id.as_ref() == "DST_BASE"
            || d.target_id.as_ref() == "BASE";
        if is_base {
            println!("  [{i}] {:?}: NO_DEFORMER (base)", d.id);
            continue;
        }
        let target_idx = id_to_idx.get(d.target_id.as_ref());
        let def_idx = match target_idx { Some(&v) => v, None => { println!("  [{i}] {:?}: target NOT FOUND", d.id); continue; }};
        let def = &data.deformers[def_idx];
        let kind = match &def.kind {
            live2d_core::moc2::DeformerKind::Warp { .. } => "WARP",
            live2d_core::moc2::DeformerKind::Rotation { .. } => "ROT",
        };
        let ds = &drawable_states[i];
        let tv = &ds.transformed_vertices;
        let vc = d.vertex_count as usize;
        let valid = tv.len() >= vc * 2;
        let (t_min, t_max) = if valid {
            let mut mn = f32::MAX; let mut mx = f32::MIN;
            for &v in tv.iter().take(vc * 2) { if v < mn { mn = v; } if v > mx { mx = v; }}
            (mn, mx)
        } else { (0.0, 0.0) };
        println!("  [{i}] {:?} → def[{def_idx}] {kind} tex={} avail={} op={:.4} tv=[{:.2}..{:.2}]",
            d.id, d.texture_no, ds.available, ds.opacity, t_min, t_max);
    }
}
