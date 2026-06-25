# Move Motion System to sctk Layer-Shell Thread

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the sctk layer-shell pet thread self-sufficient with its own motion system so the main X11 window can be hidden (via `set_visible(false)`) on compositors that don't support minimize (niri, river, sway), while keeping the model animated.

**Architecture:** The sctk thread (`wayland_pet.rs`) already loads the V3 model, creates a GL context, and renders at 60fps. Currently it receives parameter values from the main thread each frame (`PetCommand::SetParameters`). We move the full motion system (motion queue, eye blink, breath, look, expression, pose, physics) into the sctk thread. The sctk thread loads motion data from `model3.json` references during setup, advances motion each frame, and renders directly. V2 models (handled via C++ SDK internally) are already self-sufficient in the sctk thread and need no changes.

**Tech Stack:** Rust, winit, sctk (smithay-client-toolkit), wayland-protocols (zwlr_layer_shell_v1)

---

## File Structure

| File | Role |
|------|------|
| `live2d-viewer/src/wayland_pet.rs` | Main target — add motion loading + advancement to V3 path |
| `live2d-viewer/src/app.rs` | Reference only — motion loading code in `complete_v3_switch` to copy |
| `live2d-viewer/src/main.rs` | Remove `SetParameters` sync in AlwaysOnTop; remove `CursorMoved` → main thread look feed |
| `live2d-viewer/src/motion/*` | No changes — all motion modules are thread-safe (data-only, no GL or window refs) |

## Interfaces

### PetModel::V3 (after change)

```rust
PetModel::V3 {
    _moc: live2d_core::Moc,
    model: live2d_core::Model<'static>,
    renderer: crate::renderer::Live2dRenderer,
    camera: crate::camera::Camera,
    look: crate::motion::look::Look,
    param_lookup: HashMap<String, usize>,
    // NEW fields:
    motion_queue: crate::motion::MotionQueueManager,
    expression_manager: crate::motion::ExpressionManager,
    eye_blink: crate::motion::eye_blink::EyeBlink,
    breath: crate::motion::breath::Breath,
    loaded_motions: HashMap<String, Vec<crate::motion::CubismMotion>>,
    loaded_expressions: HashMap<String, crate::motion::ExpressionMotion>,
    eye_blink_param_ids: Vec<String>,
    lip_sync_param_ids: Vec<String>,
    auto_play_idle: bool,
    tap_count: usize,
    pose_data: Option<crate::model_loader::PoseData>,
    pose_fade_remaining: f32,
    part_ids: Vec<String>,
    physics: Option<crate::motion::physics::PhysicsEngine>,
    parameter_defaults: Vec<f32>,
    parameter_mins: Vec<f32>,
    parameter_maxs: Vec<f32>,
    parameter_names: Vec<String>,
}
```

### PetCommand changes

Remove `SetParameters` variant. Keep `Enter`, `SetClickThrough`, `Exit`.

### PetEvent changes

Keep all existing variants. The `Tap` event is no longer sent for V3 (handled locally in sctk thread). Keep for potential future use or V2 forwarding (unchanged).

---

### Task 1: Add motion-data loading to sctk thread V3 setup

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs:578-585` (PetModel::V3 construction region)

**Context:** In `setup_pet_surface`, after line 576 (the `let param_lookup: ...` block), and before the `PetModel::V3 { ... }` construction, we need to extract parameter metadata, groups, and load motions/expressions/pose/physics/hit_areas from the already-read `model3_json` and `model` objects.

The `model3_json` variable (type `crate::model_loader::Model3Json`) is already available. The `base_dir` variable (`PathBuf`) is available at line 508. The `model` variable is available at line 522.

After these steps, construct `PetModel::V3` with all the new fields. Initialize `motion_queue`, `expression_manager`, `eye_blink`, `breath` with `::new()` (same defaults as `app.rs`). Set `auto_play_idle = true`, `tap_count = 0`, `pose_fade_remaining = 0.0`.

**V3 idle motion auto-start:** After constructing `PetModel::V3`, if `auto_play_idle && loaded_motions.contains_key("Idle")`, call `motion_queue.start_motion(loaded_motions["Idle"][0].clone())`.

- [ ] **Step 1: Verify MotionRef type accessibility**

Check that `crate::model_loader::MotionRef` is publicly accessible (the `file`, `fade_in`, `fade_out` fields need to be read). Run:

```bash
cd live2d-viewer && cargo doc --document-private-items -p live2d-viewer 2>&1 | grep -i motionref || true
grep -n "pub struct MotionRef" src/model_loader.rs
```

Expected: the struct should be public. If not, make `pub struct MotionRef` public (and its fields `pub`).

- [ ] **Step 2: Verify ExpressionRef accessibility**

Check that `crate::model_loader::ExpressionRef` fields (`file`) are accessible:

```bash
grep -n "pub struct ExpressionRef" src/model_loader.rs
```

If not public, add `pub` to the struct and its fields.

- [ ] **Step 3: Build parameter metadata and part IDs**

After the existing `let param_lookup: HashMap<...> = ...` block (around line 576) and before `PetModel::V3` construction, add:

```rust
// Parameter metadata (for motion evaluation, physics)
let parameter_names: Vec<String> = model
    .parameters()
    .ids()
    .iter()
    .map(|id| id.to_string_lossy().into_owned())
    .collect();
let parameter_mins: Vec<f32> = model.parameters().minimum_values().to_vec();
let parameter_maxs: Vec<f32> = model.parameters().maximum_values().to_vec();
let parameter_defaults: Vec<f32> = model.parameters().default_values().to_vec();

// Part IDs (for PartOpacity motion curves)
let part_ids: Vec<String> = model
    .parts()
    .ids()
    .iter()
    .map(|id| id.to_string_lossy().into_owned())
    .collect();
```

- [ ] **Step 4: Parse groups (eye blink, lip sync)**

```rust
// Groups from model3.json
let mut eye_blink_param_ids: Vec<String> = Vec::new();
let mut lip_sync_param_ids: Vec<String> = Vec::new();
if let Some(ref groups) = model3_json.groups {
    for group in groups {
        match group.name.as_str() {
            "EyeBlink" => eye_blink_param_ids = group.ids.clone(),
            "LipSync" => lip_sync_param_ids = group.ids.clone(),
            _ => {}
        }
    }
}
```

- [ ] **Step 5: Parse motions from model3.json references**

```rust
let mut loaded_motions: HashMap<String, Vec<crate::motion::CubismMotion>> = HashMap::new();
if let Some(ref motions_map) = model3_json.file_references.motions {
    for (category, refs) in motions_map {
        let mut motions: Vec<crate::motion::CubismMotion> = Vec::new();
        for motion_ref in refs {
            let motion_path = base_dir.join(&motion_ref.file);
            match std::fs::read(&motion_path) {
                Ok(bytes) => {
                    match crate::motion::json::parse_motion_json(&bytes) {
                        Ok(parsed) => {
                            let fi = motion_ref.fade_in.unwrap_or(-1.0);
                            let fo = motion_ref.fade_out.unwrap_or(-1.0);
                            motions.push(crate::motion::CubismMotion::new(parsed, fi, fo));
                        }
                        Err(e) => log::warn!("[pet/wayland] parse motion {}: {e}", category),
                    }
                }
                Err(e) => log::warn!("[pet/wayland] read motion file {motion_path:?}: {e}"),
            }
        }
        if !motions.is_empty() {
            loaded_motions.insert(category.clone(), motions);
        }
    }
}
```

- [ ] **Step 6: Parse expressions**

```rust
let mut loaded_expressions: HashMap<String, crate::motion::ExpressionMotion> = HashMap::new();
if let Some(ref expr_refs) = model3_json.file_references.expressions {
    for expr_ref in expr_refs {
        let expr_path = base_dir.join(&expr_ref.file);
        match std::fs::read(&expr_path) {
            Ok(bytes) => {
                match crate::motion::json::parse_expression_json(&bytes) {
                    Ok(parsed) => {
                        let name = expr_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        loaded_expressions.insert(name, crate::motion::ExpressionMotion::new(parsed));
                    }
                    Err(e) => log::warn!("[pet/wayland] parse expression {expr_path:?}: {e}"),
                }
            }
            Err(e) => log::warn!("[pet/wayland] read expression {expr_path:?}: {e}"),
        }
    }
}
```

- [ ] **Step 7: Parse pose data**

```rust
let mut pose_data: Option<crate::model_loader::PoseData> = None;
if let Some(ref pose_file) = model3_json.file_references.pose {
    let pose_path = base_dir.join(pose_file);
    match std::fs::read(&pose_path) {
        Ok(bytes) => {
            match crate::model_loader::parse_pose_json(&bytes) {
                Ok(p) => {
                    log::info!("[pet/wayland] loaded pose ({} groups)", p.groups.len());
                    pose_data = Some(p);
                }
                Err(e) => log::warn!("[pet/wayland] parse pose {pose_path:?}: {e}"),
            }
        }
        Err(e) => log::warn!("[pet/wayland] read pose {pose_path:?}: {e}"),
    }
}
```

- [ ] **Step 8: Parse physics**

```rust
let mut physics: Option<crate::motion::physics::PhysicsEngine> = None;
if let Some(ref physics_file) = model3_json.file_references.physics {
    let physics_path = base_dir.join(physics_file);
    match std::fs::read(&physics_path) {
        Ok(bytes) => {
            match crate::motion::physics::PhysicsEngine::from_json(&bytes) {
                Ok(mut engine) => {
                    log::info!("[pet/wayland] loaded physics ({} sub-rigs)", engine.sub_rig_count());
                    // Apply stabilization to reset physics to default state
                    let mut params = crate::motion::physics::PhysicsParams {
                        values: parameter_defaults.clone().as_mut_slice(),
                        minimums: &parameter_mins,
                        maximums: &parameter_maxs,
                        defaults: &parameter_defaults,
                        names: &parameter_names,
                    };
                    engine.stabilization(&mut params);
                    // Write stabilized params back to model
                    {
                        let mut mp = model.parameters();
                        let mut vals = mp.values_mut();
                        let len = vals.len().min(parameter_defaults.len());
                        vals[..len].copy_from_slice(&parameter_defaults[..len]);
                    }
                    physics = Some(engine);
                }
                Err(e) => log::warn!("[pet/wayland] parse physics {physics_path:?}: {e}"),
            }
        }
        Err(e) => log::warn!("[pet/wayland] read physics {physics_path:?}: {e}"),
    }
}
```

- [ ] **Step 9: Store hit areas**

```rust
let hit_areas: Vec<crate::model_loader::HitArea> = model3_json
    .hit_areas
    .clone()
    .unwrap_or_default();
```

- [ ] **Step 10: Construct PetModel::V3 with new fields and start idle motion**

Replace the existing `PetModel::V3 { ... }` construction to include all new fields. After construction, start idle motion:

```rust
let auto_play_idle = true;
let mut motion_queue = crate::motion::MotionQueueManager::new();
if auto_play_idle {
    if let Some(idle_motions) = loaded_motions.get("Idle") {
        if let Some(first) = idle_motions.first() {
            motion_queue.start_motion(first.clone());
            log::info!("[pet/wayland] started idle motion");
        }
    }
}

PetModel::V3 {
    _moc: moc,
    model,
    renderer,
    camera,
    look: crate::motion::look::Look::new(),
    param_lookup,
    // New:
    motion_queue,
    expression_manager: crate::motion::ExpressionManager::new(),
    eye_blink: crate::motion::eye_blink::EyeBlink::new(),
    breath: crate::motion::breath::Breath::new(),
    loaded_motions,
    loaded_expressions,
    eye_blink_param_ids,
    lip_sync_param_ids,
    auto_play_idle,
    tap_count: 0usize,
    pose_data,
    pose_fade_remaining: 0.0f32,
    part_ids,
    physics,
    parameter_defaults,
    parameter_mins,
    parameter_maxs,
    parameter_names,
    hit_areas,
}
```

- [ ] **Step 11: Build and fix compile errors**

```bash
cargo check --release -p live2d-viewer 2>&1 | head -60
```

Fix any type mismatches or missing imports. Likely issues:
- `std::collections::HashMap` import needed
- `model3_json` lifetime / borrow issues (use `.clone()` if needed)
- Ensure `MotionRef` and `ExpressionRef` fields are accessible

---

### Task 2: Run motion system in sctk thread's frame loop

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs:731-838` (the V3 rendering block in `run_event_loop`)

**Context:** Currently the V3 branch in `run_event_loop` (line 731) does:
1. Local look-at tracking (pointer → NDC → set_target → subtract old offset → compute → add new offset)
2. `model.update()` (Cubism SDK physics)
3. Forward tap to main thread (line 780-791)
4. `renderer.render(&gl, model, camera)` (line 793-795)
5. Receives `SetParameters` from main thread (line 704-718, replaced above)

In the new architecture, the V3 branch will:
1. Advance motion system (full `advance_motion` equivalent)
2. Call `model.update()`
3. Handle V3 tap locally (hit test → start TapBody motion)
4. Render
5. No `SetParameters` needed (motion runs locally)

- [ ] **Step 1: Replace the V3 frame body**

Replace the V3 match arm in `run_event_loop` (lines 731-796) with:

```rust
PetModel::V3 {
    model,
    renderer,
    camera,
    look,
    param_lookup,
    motion_queue,
    expression_manager,
    eye_blink,
    breath,
    loaded_motions,
    loaded_expressions,
    eye_blink_param_ids,
    lip_sync_param_ids,
    auto_play_idle,
    tap_count,
    pose_data,
    pose_fade_remaining,
    part_ids,
    physics,
    parameter_defaults,
    parameter_mins,
    parameter_maxs,
    parameter_names,
    hit_areas,
    ..
} => {
    let mut pl = param_lookup;
    let mut pn = parameter_names;
    let mut pmins = parameter_mins;
    let mut pmaxs = parameter_maxs;
    let mut pdefs = parameter_defaults;
    let mut pid = part_ids;
    let mut eb_ids = eye_blink_param_ids;
    let mut ls_ids = lip_sync_param_ids;
    let mut ha = hit_areas;
    let mut ap = auto_play_idle;

    // == Frame timing (used by look + motion) ==
    let now = std::time::Instant::now();
    let dt = (now - prev_look_time).as_secs_f32().min(0.1);
    prev_look_time = now;

    // == Update local look controller (from overlay cursor) ==
    {
        let (px, py) = (state.ptr.pointer_x as f32, state.ptr.pointer_y as f32);
        let (cw, ch) = (size.0 as f32, size.1 as f32);
        let ndc_x = 2.0 * px / cw - 1.0;
        let ndc_y = 1.0 - 2.0 * py / ch;
        look.set_target(ndc_x, ndc_y);

        // Subtract old look offset
        let mut params = model.parameters();
        let mut vals = params.values_mut();
        let slice = vals.as_mut_slice();
        for p in &look.params {
            if let Some(&idx) = param_lookup.get(&p.id) {
                if idx < slice.len() {
                    slice[idx] -= p.current_offset;
                }
            }
        }
    }

    // == Advance motion system ==
    motion_queue.advance_time(dt);

    // Read current part opacities for PartOpacity curve evaluation
    let mut motion_part_opacities: Vec<f32> = model.parts().opacities().to_vec();

    // Evaluate motion curves
    let mut param_values: Vec<f32> = model.parameters().values().to_vec();
    motion_queue.do_update_motion(
        &parameter_names,
        param_lookup,
        &mut param_values,
        &eye_blink_param_ids,
        &lip_sync_param_ids,
        &part_ids,
        &mut motion_part_opacities,
    );

    // Write motion-updated part opacities back to model
    if !motion_part_opacities.is_empty() {
        let mut parts = model.parts();
        let opacities = parts.opacities_mut();
        let len = opacities.len().min(motion_part_opacities.len());
        opacities[..len].copy_from_slice(&motion_part_opacities[..len]);
    }

    // Apply expression (if active)
    expression_manager.apply(
        &parameter_names,
        &mut param_values,
        motion_queue.user_time_seconds,
    );

    // Apply EyeBlink
    if !eye_blink_param_ids.is_empty() {
        let blink = eye_blink.update(dt);
        if (blink - 1.0).abs() > 1e-6 {
            for id in &eye_blink_param_ids {
                if let Some(&idx) = param_lookup.get(id) {
                    param_values[idx] = blink;
                }
            }
        }
    }

    // Apply Breath (delta-additive oscillation)
    breath.update(dt, &mut param_values, param_lookup);

    // Apply look: compute new offset, add back
    look.compute_raw(dt);
    for p in &look.params {
        if let Some(&idx) = param_lookup.get(&p.id) {
            if idx < param_values.len() {
                param_values[idx] += p.current_offset;
            }
        }
    }

    // Apply Physics
    if let Some(ref mut engine) = physics {
        let mut params = crate::motion::physics::PhysicsParams {
            values: &mut param_values,
            minimums: &parameter_mins,
            maximums: &parameter_maxs,
            defaults: &parameter_defaults,
            names: &parameter_names,
        };
        engine.evaluate(&mut params, dt);
    }

    // Auto-restart Idle when all motions finished
    if *auto_play_idle && motion_queue.entries.is_empty() {
        if let Some(idle_motions) = loaded_motions.get("Idle") {
            if let Some(first) = idle_motions.first() {
                motion_queue.start_motion(first.clone());
            }
        }
    }

    // Write computed parameter values to the Cubism model
    {
        let mut params = model.parameters();
        let mut vals = params.values_mut();
        let len = vals.len().min(param_values.len());
        vals[..len].copy_from_slice(&param_values[..len]);
    }

    // Update pose (part opacity copying)
    if let Some(ref pose) = pose_data {
        let mut parts = model.parts();
        let pids: Vec<String> = parts
            .ids()
            .iter()
            .map(|id| id.to_string_lossy().into_owned())
            .collect();
        let popac = parts.opacities_mut();
        for group in &pose.groups {
            for entry in group {
                if entry.links.is_empty() {
                    continue;
                }
                if let Some(main_idx) = pids.iter().position(|id| id == &entry.id) {
                    let opacity = popac[main_idx];
                    for link_id in &entry.links {
                        if let Some(link_idx) = pids.iter().position(|id| id == link_id) {
                            popac[link_idx] = opacity;
                        }
                    }
                }
            }
        }
    }

    // == Run Cubism SDK update ==
    model.update();

    // == Handle V3 tap locally (hit test → start TapBody motion) ==
    if let Some((cx, cy)) = state.ptr.pending_click.take() {
        let ndc_x = 2.0 * cx as f32 / size.0 as f32 - 1.0;
        let ndc_y = 1.0 - 2.0 * cy as f32 / size.1 as f32;
        let model_x = (ndc_x - camera.translate_x) / camera.scale_x;
        let model_y = (ndc_y - camera.translate_y) / camera.scale_y;

        let drawables = model.drawables();
        let drawable_ids = drawables.ids();
        let vpos = drawables.vertex_positions();
        let vcounts = drawables.vertex_counts();
        let idxs_arr = drawables.indices();
        let icounts = drawables.index_counts();

        let mut hit = false;
        for hit_area in ha {
            let di = match drawable_ids
                .iter()
                .position(|id| id.to_string_lossy() == hit_area.id)
            {
                Some(i) => i,
                None => continue,
            };

            let verts = unsafe { std::slice::from_raw_parts(vpos[di], vcounts[di] as usize) };
            let idx = unsafe { std::slice::from_raw_parts(idxs_arr[di], icounts[di] as usize) };

            for tri in idx.chunks(3) {
                if tri.len() < 3 { continue; }
                let a = &verts[tri[0] as usize];
                let b = &verts[tri[1] as usize];
                let c = &verts[tri[2] as usize];
                if crate::app::point_in_triangle(model_x, model_y, a.X, a.Y, b.X, b.Y, c.X, c.Y) {
                    if let Some(motions) = loaded_motions.get("TapBody") {
                        if !motions.is_empty() {
                            let idx = *tap_count % motions.len();
                            *tap_count += 1;
                            motion_queue.stop_all_motions();
                            let mut motion = motions[idx].clone();
                            motion.is_loop = false;
                            motion_queue.start_motion(motion);
                        }
                    }
                    hit = true;
                    break;
                }
            }
            if hit { break; }
        }
        // Always consume the pending click even if no hit
    }

    // == Render ==
    unsafe {
        renderer.render(&gl, model, camera);
    }
}
```

Note: The `pet_model` match borrows multiple fields mutably. This will require destructuring the `PetModel::V3` to take local `&mut` references. If the Rust borrow checker rejects simultaneous `model.parameters()`, `model.parts()`, `model.drawables()` calls, split them into scoped blocks (the current code already uses `{}` blocks for each access).

The above code uses `point_in_triangle` from `crate::app`. Verify this function is public; if not, change its visibility in `app.rs`:

```rust
// In app.rs, change from private to pub:
pub fn point_in_triangle(...) -> bool { ... }
```

- [ ] **Step 2: Build and fix borrow-checker errors**

```bash
cargo check --release -p live2d-viewer 2>&1 | head -80
```

Likely issues:
- Multiple mutable borrows of `model` — use scoped blocks (the pattern already used in `advance_motion` in `app.rs` accesses model.parts() and model.parameters() in sequence, separated by drop of the first borrow)
- Need `use crate::app::point_in_triangle;` or qualify with `crate::app::point_in_triangle`
- Ensure all needed `use` imports exist at top of `wayland_pet.rs`

---

### Task 3: Remove `SetParameters` from `PetCommand` and stop sending from main thread

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs` — remove `SetParameters` variant and its handler
- Modify: `live2d-viewer/src/main.rs` — remove `SetParameters` send in `RedrawRequested`

- [ ] **Step 1: Remove `SetParameters` from `PetCommand` enum**

```rust
pub enum PetCommand {
    Enter {
        model_dir: PathBuf,
        model_format: crate::app::ModelFormat,
        click_through: bool,
    },
    SetClickThrough(bool),
    Exit,
}
```

- [ ] **Step 2: Remove `SetParameters` handler in `run_event_loop`**

Remove the `PetCommand::SetParameters { values, part_opacities }` match arm (currently lines 704-720). The handler should be deleted entirely.

- [ ] **Step 3: Remove `SetParameters` send in `main.rs`**

In `main.rs`, within the `RedrawRequested` handler, remove the block that sends `SetParameters` (currently lines 559-574):

```rust
// Remove this entire block:
// Sync parameters to AlwaysOnTop overlay thread
#[cfg(target_os = "linux")]
if let Some(ref tx) = app.pet_wayland_cmd_tx {
    let values = app.parameter_values.clone();
    let part_opacities = app
        .current_model
        .as_ref()
        .map(|m| m.parts().opacities().to_vec())
        .unwrap_or_default();
    let _ = tx.send(
        crate::wayland_pet::PetCommand::SetParameters { values, part_opacities },
    );
}
```

- [ ] **Step 4: Build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

---

### Task 4: Hide main window on niri-incompatible compositors

**Files:**
- Modify: `live2d-viewer/src/main.rs:493-498` — the `set_minimized(true)` line

- [ ] **Step 1: Replace `set_minimized(true)` with `set_visible(false)`**

```rust
// Hide main window — the layer-shell overlay renders the model.
// On KDE/GNOME, set_minimized(true) hides to taskbar.
// On niri/sway/river (no minimize concept), use set_visible(false)
// to unmap the X11 window.  The event loop keeps running via
// AboutToWait → request_redraw() at line 1090.
window.set_visible(false);
```

Note: If `set_visible(false)` doesn't work on niri (confirmed during testing — XWayland may ignore UnmapWindow), the motion system is already in the sctk thread, so we can upgrade to a stronger approach: destroy the main window and rely entirely on the sctk thread for the event loop. In that case, uncomment and add this fallback:

```rust
// TODO: If set_visible(false) is also ignored by niri, destroy the window
// and keep the process alive via the sctk thread's Wayland connection.
// window.destroy() is not supported by winit 0.29, but we can keep
// the window at 1x1 in the corner as a last resort.
```

- [ ] **Step 2: Build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

---

### Task 5: Stop sending CursorMoved from sctk thread (look is now local)

**Files:**
- Modify: `live2d-viewer/src/wayland_pet.rs:279-370` (pointer event handler that sends `PetEvent::CursorMoved`)

**Context:** In the sctk thread's V3 model path, look-at-cursor tracking is now fully local (within `run_event_loop`). The main thread no longer needs `CursorMoved` events to drive the look controller for the overlay. However, `CursorMoved` events might still be useful for other purposes, so we keep the event type but stop sending it for V3 models. For V2 models, CursorMoved events were never used (V2 handles look internally via `v2_model.drag()`).

- [ ] **Step 1: Remove CursorMoved send in wl_pointer::Event::Motion handler**

In the `wl_pointer::Event::Motion` handler, find the block that sends `PetEvent::CursorMoved` (around lines 290-310). Wrap it in a check:

```rust
// For V3 models, look-at tracking is now handled locally in run_event_loop.
// Only send CursorMoved for V2 models (which handle input differently).
// Since we can't inspect the model type here, we simply skip sending
// CursorMoved for all models — the main thread doesn't need it anymore
// (both V2 and V3 handle cursor tracking locally in the pet thread).
// Keep the PetEvent::CursorMoved enum variant for API stability but
// never send it.
```

However, looking at the actual code, `CursorMoved` is sent in the pointer Motion event handler (around line 290). The main thread receives it in `AboutToWait` (line 1070) and calls `app.update_mouse_for_look(...)`. Since the sctk thread now handles look locally for V3 AND V2 already handles drag locally, the `CursorMoved` → main thread feed is redundant. Remove the send call.

But the `PetEvent::CursorMoved` variant should remain in the enum (it's a public API). Just stop sending it. Add a comment.

```rust
// CursorMoved is no longer sent to main thread — look tracking
// is handled locally in the pet thread for both V3 and V2 models.
// The enum variant is kept for API compatibility.
```

The exact code change depends on where `PetEvent::CursorMoved` is sent. Based on the code at line ~300:

Look at the pointer Motion handler (around line 290-310). Remove the block:

```rust
// Remove this send:
let _ = state.event_tx.send(PetEvent::CursorMoved {
    x: state.ptr.pointer_x,
    y: state.ptr.pointer_y,
    w: size.0 as f32,
    h: size.1 as f32,
});
```

But note: the `state.configured_size` might need to be unwrapped for the `w, h` fields. If this send is inside code that checks `state.configured_size`, that code can be simplified or removed.

- [ ] **Step 2: Remove CursorMoved handling in main.rs**

In `main.rs`, `AboutToWait` handler (around line 1070-1072), remove:

```rust
crate::wayland_pet::PetEvent::CursorMoved { x, y, w, h } => {
    app.update_mouse_for_look(x, y, w, h);
}
```

- [ ] **Step 3: Build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

---

### Task 6: Clean up and verify

**Files:**
- All modified files

- [ ] **Step 1: Remove unused imports**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Check for `unused import` warnings and remove them. Likely candidates in `wayland_pet.rs`: `SetParameters`-related code, now-unused `mpsc::Sender` usage for cursor events.

- [ ] **Step 2: Full build**

```bash
cargo check --release -p live2d-viewer 2>&1
```

Expected: clean compile, zero warnings.

- [ ] **Step 3: Review for correctness**

Manual review checklist:
- [ ] In `setup_pet_surface`, do all motion fields get initialized before constructing `PetModel::V3`?
- [ ] In `run_event_loop`, does the V3 branch use the new fields and not the old `SetParameters` path?
- [ ] In `run_event_loop`, does the V2 path remain unchanged?
- [ ] In `main.rs`, is `SetParameters` send completely removed?
- [ ] In `main.rs`, is `set_visible(false)` called instead of `set_minimized(true)`?
- [ ] In `main.rs`, is the `CursorMoved` handler removed from `AboutToWait`?
- [ ] Is `point_in_triangle` accessible from `wayland_pet.rs`? (mark `pub` in `app.rs` if needed)
- [ ] Are `MotionRef` and `ExpressionRef` fields public? (mark `pub` in `model_loader.rs` if needed)
- [ ] Does the physics stabilization write to the model parameters before the first frame?
- [ ] Is `auto_play_idle` set correctly so idle motions start on the pet overlay?

---

## Build & Verify

After all tasks:

```bash
cargo check --release -p live2d-viewer
cargo clippy --release -p live2d-viewer 2>&1 | grep -v "warning:" | head -20
cargo fmt -p live2d-viewer
```

Run with a V3 model:

```bash
LIVE2D_SDK_ROOT=/path/to/CubismSdkForNative-5-r.5 cargo run --release -p live2d-viewer -- --pet-mode=alwaysontop /path/to/model
```

Expected behavior on KDE:
- Main window minimizes to taskbar (set_minimized(true) still works there)
- Pet overlay appears at bottom-right corner
- Model animates (idle motion, eye blink, breath, look-at-cursor)

Expected behavior on niri:
- Main window disappears (set_visible(false) hides it)
- Pet overlay appears at bottom-right corner
- Model animates independently (motion runs in sctk thread)
- Tapping the overlay triggers hit test → TapBody motion

Fallback: If `set_visible(false)` also fails on niri, the sctk thread's independent motion system means we can explore destroying the X11 window or resizing it to 1x1 — the overlay animation continues regardless.
