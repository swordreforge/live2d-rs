use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use live2d_core::{Moc, Model};
use live2d_core::moc2::{Moc2Model, parse_moc2};

use crate::camera::Camera;
use crate::motion;
use crate::model_adapter::{LoadedModelVariant, Moc2ModelAdapter};

pub struct ModelEntry {
    pub name: String,
    pub dir: PathBuf,
    pub loaded: bool,
}

pub struct AppState {
    pub model_list: Vec<ModelEntry>,
    pub current_idx: Option<usize>,
    pub current_moc: Option<Moc>,
    pub current_model: Option<LoadedModelVariant>,
    pub parameter_values: Vec<f32>,
    pub parameter_names: Vec<String>,
    pub parameter_mins: Vec<f32>,
    pub parameter_maxs: Vec<f32>,
    /// Original model parameter default values (used by physics normalization)
    pub parameter_defaults: Vec<f32>,
    pub texture_paths: Vec<PathBuf>,
    pub error_message: Option<String>,
    pub mouse_down: bool,
    pub last_mouse_x: f64,
    pub last_mouse_y: f64,
    // Motion system
    pub motion_queue: motion::MotionQueueManager,
    pub expression_manager: motion::ExpressionManager,
    pub eye_blink: motion::eye_blink::EyeBlink,
    pub breath: motion::breath::Breath,
    pub look: motion::look::Look,
    /// Loaded motions by category (e.g. "idle", "tap_body")
    pub loaded_motions: HashMap<String, Vec<motion::CubismMotion>>,
    /// Loaded expressions by name
    pub loaded_expressions: HashMap<String, motion::ExpressionMotion>,
    /// Eye blink parameter IDs (from model3.json Groups)
    pub eye_blink_param_ids: Vec<String>,
    /// Lip sync parameter IDs (from model3.json Groups)
    pub lip_sync_param_ids: Vec<String>,
    /// Whether to auto-start the first idle motion
    pub auto_play_idle: bool,
    /// Base directory for resolving relative paths
    pub base_dir: Option<PathBuf>,
    pub hit_areas: Vec<crate::model_loader::HitArea>,
    tap_count: usize,
    pub pose_data: Option<crate::model_loader::PoseData>,
    pub pose_fade_remaining: f32,
    /// Part IDs from the current model (for PartOpacity motion curves)
    pub part_ids: Vec<String>,
    /// Desktop pet mode: transparent, frameless, minimal UI
    pub pet_mode: bool,
    /// Set to true when pet_mode toggles so main.rs applies window changes
    pub pet_mode_changed: bool,
    /// True when camera needs recalculation (after pet mode window resize)
    pub camera_needs_fit: bool,
    /// Frame delay before showing pet toolbar (let window resize settle)
    pub pet_mode_delay: u32,
    /// True when pet mode needs window resize (after model switch)
    pub pet_resize_pending: bool,
    /// Request minimize to floating circle
    pub request_minimize: bool,
    /// Request restore from floating circle
    pub request_restore: bool,
    /// True when window is minimized to a floating overlay
    pub minimized_to_float: bool,
    /// Saved pet mode window size (logical pixels) for restore
    pub saved_window_pet_size: (f64, f64),
    /// Pre-built parameter name → index lookup (built once at model load)
    pub param_lookup: HashMap<String, usize>,
    /// Camera (view transform for the model)
    pub camera: Camera,
    /// Current window size in pixels (set from main.rs each frame)
    pub window_size: (f32, f32),
    /// Model canvas pixel size (from canvas_info), for stable toolbar positioning
    pub canvas_pixel_size: (f32, f32),
    /// Physics engine loaded from physics3.json
    pub physics: Option<motion::physics::PhysicsEngine>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            model_list: Vec::new(),
            current_idx: None,
            current_moc: None,
            current_model: None,
            parameter_values: Vec::new(),
            parameter_names: Vec::new(),
            parameter_mins: Vec::new(),
            parameter_maxs: Vec::new(),
            parameter_defaults: Vec::new(),
            texture_paths: Vec::new(),
            error_message: None,
            mouse_down: false,
            last_mouse_x: 0.0,
            last_mouse_y: 0.0,
            motion_queue: motion::MotionQueueManager::new(),
            expression_manager: motion::ExpressionManager::new(),
            eye_blink: motion::eye_blink::EyeBlink::new(),
            breath: motion::breath::Breath::new(),
            look: motion::look::Look::new(),
            loaded_motions: HashMap::new(),
            loaded_expressions: HashMap::new(),
            eye_blink_param_ids: Vec::new(),
            lip_sync_param_ids: Vec::new(),
            auto_play_idle: true,
            base_dir: None,
            hit_areas: Vec::new(),
            tap_count: 0,
            pose_data: None,
            pose_fade_remaining: 0.0,
            part_ids: Vec::new(),
            pet_mode: false,
            pet_mode_changed: false,
            camera_needs_fit: false,
            pet_mode_delay: 0,
            pet_resize_pending: false,
            request_minimize: false,
            request_restore: false,
            minimized_to_float: false,
            saved_window_pet_size: (0.0, 0.0),
            param_lookup: HashMap::new(),
            camera: Camera::new(),
            window_size: (800.0, 600.0),
            canvas_pixel_size: (0.0, 0.0),
            physics: None,
        }
    }

    pub fn add_model_dir(&mut self, path: PathBuf) {
        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        self.model_list.push(ModelEntry { name, dir: path, loaded: false });
    }

    pub fn switch_to(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.model_list.len() {
            return Err("index out of range".into());
        }

        // Drop order: Model variant first, then Moc
        self.current_model = None;
        self.current_moc = None;
        self.parameter_values.clear();
        self.parameter_names.clear();
        self.parameter_mins.clear();
        self.parameter_maxs.clear();
        self.loaded_motions.clear();
        self.loaded_expressions.clear();
        self.eye_blink_param_ids.clear();
        self.lip_sync_param_ids.clear();
        self.hit_areas.clear();
        self.pose_data = None;
        self.physics = None;
        self.motion_queue.stop_all_motions();

        let entry = &self.model_list[idx];

        // Detect MOC format: .moc (Cubism 2.x) vs .moc3 (Cubism 3+)
        // Look for a model3.json or model.json in the directory
        let model3_path = entry.dir.join(format!("{}.model3.json", entry.name));
        let model2_path = entry.dir.join("model.json");

        if model3_path.exists() {
            // ── Cubism 3+ (Core) path ──
            let loaded = crate::model_loader::LoadedModel::load(&entry.dir)
                .map_err(|e| format!("load model: {e}"))?;

            let moc = Moc::revive(&loaded.moc3_data)
                .map_err(|e| format!("revive moc: {e}"))?;

            let moc_ptr: *const Moc = &moc as *const Moc;
            let model = unsafe { Model::initialize(&*moc_ptr) }
                .map_err(|e| format!("init model: {e}"))?;
            let model: Model<'static> = unsafe { std::mem::transmute(model) };

            let params = model.parameters();
            self.parameter_names = params.ids().iter()
                .map(|id| id.to_string_lossy().into_owned())
                .collect();
            self.parameter_values = params.default_values().to_vec();
            self.parameter_mins = params.minimum_values().to_vec();
            self.parameter_maxs = params.maximum_values().to_vec();
            self.parameter_defaults = params.default_values().to_vec();

            self.part_ids = model.parts().ids().iter()
                .map(|id| id.to_string_lossy().into_owned())
                .collect();

            let canvas = model.canvas_info();
            self.canvas_pixel_size = (canvas.size_in_pixels.X, canvas.size_in_pixels.Y);

            self.current_moc = Some(moc);
            self.current_model = Some(LoadedModelVariant::Core(model));
            self.texture_paths = loaded.texture_paths();
            self.base_dir = Some(loaded.base_dir.clone());

            // Load metadata
            if let Some(ref groups) = loaded.model3_json.groups {
                for group in groups {
                    match group.name.as_str() {
                        "EyeBlink" => self.eye_blink_param_ids = group.ids.clone(),
                        "LipSync" => self.lip_sync_param_ids = group.ids.clone(),
                        _ => {}
                    }
                }
            }
            if let Some(ref areas) = loaded.model3_json.hit_areas {
                self.hit_areas = areas.clone();
            }

            self.load_all_motions(&loaded.base_dir, &loaded.model3_json);
            self.load_all_expressions(&loaded.base_dir, &loaded.model3_json);
            self.load_pose(&loaded.base_dir, &loaded.model3_json);
            self.apply_pose_reset();
            self.load_physics(&loaded.base_dir, &loaded.model3_json);
        } else if model2_path.exists() || entry.dir.join(format!("{}.model.json", entry.name)).exists() || entry.dir.join(format!("{}.moc", entry.name)).exists() {
            // ── Cubism 2.x (MOC2) path ──
            // Find the .moc file
            let moc_file = entry.dir.join(format!("{}.moc", entry.name));
            let moc_data = std::fs::read(&moc_file)
                .map_err(|e| format!("read .moc: {e}"))?;

            let moc2_data = parse_moc2(&moc_data)
                .map_err(|e| format!("parse MOC2: {e}"))?;
            let moc2_data = Arc::new(moc2_data);

            let runtime = Moc2Model::new(moc2_data.clone());
            let adapter = Moc2ModelAdapter::new(runtime, moc2_data.clone());

            // Populate param metadata
            for p in adapter.data.param_defs.iter() {
                self.parameter_names.push(p.id.to_string());
                self.parameter_values.push(p.default_value);
                self.parameter_mins.push(p.min_value);
                self.parameter_maxs.push(p.max_value);
                self.parameter_defaults.push(p.default_value);
            }

            // Part IDs
            self.part_ids = adapter.data.parts.iter()
                .map(|p| p.id.to_string())
                .collect();

            let canvas = adapter.canvas_info();
            self.canvas_pixel_size = (canvas.size_in_pixels.X, canvas.size_in_pixels.Y);

            // ── Load textures from model.json if available (ordered list) ──
            // model.json can be "model.json" or "{name}.model.json"
            let model_json_path = if model2_path.exists() {
                Some(model2_path)
            } else {
                let candidate = entry.dir.join(format!("{}.model.json", entry.name));
                if candidate.exists() { Some(candidate) } else { None }
            };

            let moc2_json = model_json_path.as_ref().and_then(|p| {
                crate::model_loader::Moc2ModelJson::from_file(p).ok()
            });

            if let Some(ref json) = moc2_json {
                // Load textures from ordered list
                self.texture_paths = json.texture_paths(&entry.dir);
                log::info!("Loaded {} textures from MOC2 model.json", self.texture_paths.len());
            }

            // Fallback: try single PNG with same name as .moc, or any PNG in directory
            if self.texture_paths.is_empty() {
                let png_candidate = moc_file.with_extension("png");
                if png_candidate.exists() {
                    self.texture_paths.push(png_candidate);
                } else {
                    if let Ok(rd) = std::fs::read_dir(&entry.dir) {
                        for e in rd.flatten() {
                            let p = e.path();
                            if p.extension().map(|ext| ext == "png").unwrap_or(false) {
                                self.texture_paths.push(p);
                                break;
                            }
                        }
                    }
                }
            }

            self.current_moc = None;
            self.current_model = Some(LoadedModelVariant::V2(Box::new(adapter)));
            self.base_dir = Some(entry.dir.clone());

            // ── Load motions, expressions, physics from model.json ──
            if let Some(ref json) = moc2_json {
                let base_dir = entry.dir.clone();

                // Load motions by category
                for (category, motion_refs) in &json.motions {
                    let mut motions: Vec<crate::motion::CubismMotion> = Vec::new();
                    for mref in motion_refs {
                        let path = base_dir.join(&mref.file);
                        let data = match std::fs::read(&path) {
                            Ok(d) => d,
                            Err(e) => {
                                log::warn!("Failed to read MOC2 motion {}: {e}", path.display());
                                continue;
                            }
                        };
                        let parsed = match crate::model_loader::parse_mtn_motion(&data) {
                            Ok(p) => p,
                            Err(e) => {
                                log::warn!("Failed to parse MOC2 motion {}: {e}", path.display());
                                continue;
                            }
                        };
                        let fade_in = mref.fade_in.unwrap_or(-1.0);
                        let fade_out = mref.fade_out.unwrap_or(-1.0);
                        let cm = crate::motion::CubismMotion::new(parsed, fade_in, fade_out);
                        motions.push(cm);
                    }
                    self.loaded_motions.insert(category.clone(), motions);
                }

                // Load expressions
                for eref in &json.expressions {
                    let path = base_dir.join(&eref.file);
                    let data = match std::fs::read(&path) {
                        Ok(d) => d,
                        Err(e) => {
                            log::warn!("Failed to read MOC2 expression {}: {e}", path.display());
                            continue;
                        }
                    };
                    let parsed = match crate::model_loader::parse_moc2_expression_json(&data) {
                        Ok(p) => p,
                        Err(e) => {
                            log::warn!("Failed to parse MOC2 expression {}: {e}", path.display());
                            continue;
                        }
                    };
                    let mut em = crate::motion::ExpressionMotion::new(parsed);

                    // Extract fade_in from the JSON if present (in milliseconds → seconds)
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&content) {
                            if let Some(fade_in_ms) = root.get("fade_in").and_then(|v| v.as_f64()) {
                                em.fade_in_seconds = (fade_in_ms as f32) / 1000.0;
                            }
                        }
                    }

                    self.loaded_expressions.insert(eref.name.clone(), em);
                }

                // Load physics (MOC2 format: physics_hair → physics3.json conversion)
                if let Some(physics_file) = &json.physics {
                    let path = base_dir.join(physics_file);
                    match std::fs::read(&path) {
                        Ok(buf) => {
                            match crate::model_loader::convert_moc2_physics_to_physics3_json(&buf)
                                .and_then(|converted| {
                                    motion::physics::PhysicsEngine::from_json(&converted)
                                }) {
                                Ok(engine) => {
                                    let mut params = motion::physics::PhysicsParams {
                                        values: &mut self.parameter_values,
                                        minimums: &self.parameter_mins,
                                        maximums: &self.parameter_maxs,
                                        defaults: &self.parameter_defaults,
                                        names: &self.parameter_names,
                                    };
                                    let mut engine = engine;
                                    let sub_rigs = engine.sub_rig_count();
                                    engine.stabilization(&mut params);
                                    self.physics = Some(engine);
                                    log::info!("Loaded MOC2 physics ({sub_rigs} sub-rigs)");
                                }
                                Err(e) => {
                                    log::warn!("Failed to parse MOC2 physics {}: {e}", path.display());
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to read MOC2 physics {}: {e}", path.display());
                        }
                    }
                }

                // Load hit areas
                if !json.hit_areas.is_empty() {
                    self.hit_areas = json.hit_areas.iter().map(|ha| {
                        crate::model_loader::HitArea {
                            id: ha.id.clone(),
                            name: ha.name.clone(),
                        }
                    }).collect();
                }
            }
        } else {
            return Err("No model3.json or .moc file found".into());
        }

        // Build param lookup once — reused every frame
        self.param_lookup.clear();
        self.param_lookup.reserve(self.parameter_names.len());
        for (i, name) in self.parameter_names.iter().enumerate() {
            self.param_lookup.insert(name.clone(), i);
        }

        self.current_idx = Some(idx);
        self.model_list[idx].loaded = true;

        // Start the first idle motion (only for Core models that have motion files)
        if self.auto_play_idle {
            if let Some(idle_motions) = self.loaded_motions.get("Idle") {
                if let Some(first) = idle_motions.first() {
                    self.motion_queue.start_motion(first.clone());
                    log::info!("Started idle motion");
                }
            }
        }

        Ok(())
    }

    /// Load all motion files referenced in model3.json.
    fn load_all_motions(
        &mut self,
        base_dir: &std::path::Path,
        model3_json: &crate::model_loader::Model3Json,
    ) {
        let motions_ref = match &model3_json.file_references.motions {
            Some(m) => m,
            None => return,
        };

        for (category, refs) in motions_ref {
            let mut motions: Vec<motion::CubismMotion> = Vec::new();
            for motion_ref in refs {
                let path = base_dir.join(&motion_ref.file);
                let data = match std::fs::read(&path) {
                    Ok(d) => d,
                    Err(e) => {
                        log::warn!("Failed to read motion {}: {e}", motion_ref.file);
                        continue;
                    }
                };

                let parsed = match motion::json::parse_motion_json(&data) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("Failed to parse motion {}: {e}", motion_ref.file);
                        continue;
                    }
                };

                let fade_in = motion_ref.fade_in_time.unwrap_or(-1.0) as f32;
                let fade_out = motion_ref.fade_out_time.unwrap_or(-1.0) as f32;
                let cm = motion::CubismMotion::new(parsed, fade_in, fade_out);
                motions.push(cm);
            }
            self.loaded_motions.insert(category.clone(), motions);
        }
    }

    /// Load all expression files referenced in model3.json.
    fn load_all_expressions(
        &mut self,
        base_dir: &std::path::Path,
        model3_json: &crate::model_loader::Model3Json,
    ) {
        let exprs_ref = match &model3_json.file_references.expressions {
            Some(e) => e,
            None => return,
        };

        for exp_ref in exprs_ref {
            let path = base_dir.join(&exp_ref.file);
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("Failed to read expression {}: {e}", exp_ref.file);
                    continue;
                }
            };

            let parsed = match motion::json::parse_expression_json(&data) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("Failed to parse expression {}: {e}", exp_ref.file);
                    continue;
                }
            };

            let em = motion::ExpressionMotion::new(parsed);
            self.loaded_expressions.insert(exp_ref.name.clone(), em);
        }
    }

    fn load_pose(
        &mut self,
        base_dir: &std::path::Path,
        model3_json: &crate::model_loader::Model3Json,
    ) {
        let pose_path = match model3_json.file_references.pose {
            Some(ref p) => base_dir.join(p),
            None => return,
        };
        let data = match std::fs::read(&pose_path) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("Failed to read pose {}: {e}", pose_path.display());
                return;
            }
        };
        let parsed = match crate::model_loader::parse_pose_json(&data) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("Failed to parse pose: {e}");
                return;
            }
        };
        log::info!("Loaded pose with {} groups (fade={:.2}s)", parsed.groups.len(), parsed.fade_in_time);
        self.pose_data = Some(parsed);
    }

    /// Load physics3.json and initialize the physics engine.
    fn load_physics(
        &mut self,
        base_dir: &std::path::Path,
        model3_json: &crate::model_loader::Model3Json,
    ) {
        let physics_path = match model3_json.file_references.physics {
            Some(ref p) => base_dir.join(p),
            None => return,
        };
        let data = match std::fs::read(&physics_path) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("Failed to read physics {}: {e}", physics_path.display());
                return;
            }
        };
        let mut engine = match motion::physics::PhysicsEngine::from_json(&data) {
            Ok(e) => e,
            Err(e) => {
                log::warn!("Failed to parse physics: {e}");
                return;
            }
        };
        log::info!("Loaded physics ({} sub-rigs)", engine.sub_rig_count());

        // Run stabilization to set initial steady state
        {
            let mut params = motion::physics::PhysicsParams {
                values: &mut self.parameter_values,
                minimums: &self.parameter_mins,
                maximums: &self.parameter_maxs,
                defaults: &self.parameter_defaults,
                names: &self.parameter_names,
            };
            engine.stabilization(&mut params);
        }

        self.physics = Some(engine);
    }

    fn apply_pose_reset(&mut self) {
        let pose = match self.pose_data {
            Some(ref p) => p.clone(),
            None => return,
        };
        let model = match self.current_model {
            Some(ref mut m) => m,
            None => return,
        };
        let pids: Vec<String> = model.part_ids();
        let mut popac = model.part_opacities_mut();
        let opacities = popac.as_mut_slice();

        for group in &pose.groups {
            let mut first_found = false;
            for entry in group {
                if let Some(part_idx) = pids.iter().position(|id| id == &entry.id) {
                    if !first_found {
                        opacities[part_idx] = 1.0;
                        first_found = true;
                    } else {
                        opacities[part_idx] = 0.0;
                    }
                }
            }
        }
        // popac dropped here — propagate part opacities to drawables
        model.update();
        self.pose_fade_remaining = 0.0;
    }

    pub fn update_pose(&mut self, _delta_time: f32) {
        let pose = match self.pose_data {
            Some(ref p) => p,
            None => return,
        };
        let model = match self.current_model {
            Some(ref mut m) => m,
            None => return,
        };
        let pids: Vec<String> = model.part_ids();
        let mut popac = model.part_opacities_mut();

        // CopyPartOpacities: for any entry with links, copy main opacity to linked parts
        for group in &pose.groups {
            for entry in group {
                if entry.links.is_empty() { continue; }
                if let Some(main_idx) = pids.iter().position(|id| id == &entry.id) {
                    let opacity = popac.as_mut_slice()[main_idx];
                    for link_id in &entry.links {
                        if let Some(link_idx) = pids.iter().position(|id| id == link_id) {
                            popac.as_mut_slice()[link_idx] = opacity;
                        }
                    }
                }
            }
        }
    }

    /// Start a motion from a specific category (e.g. "idle", "tap_body").
    /// If `index` is provided, plays that specific motion; otherwise plays the first one.
    pub fn start_motion(&mut self, category: &str, index: Option<usize>) -> bool {
        let motions = match self.loaded_motions.get(category) {
            Some(m) => m,
            None => return false,
        };
        let idx = index.unwrap_or(0);
        if idx >= motions.len() {
            return false;
        }
        self.motion_queue.start_motion(motions[idx].clone());
        true
    }

    pub fn update_parameters(&mut self) {
        if let Some(ref mut model) = self.current_model {
            model.set_param_values(&self.parameter_values);
        }
    }

    /// Advance the motion system and apply to parameter values.
    /// Call this each frame before `update_parameters`.
    pub fn advance_motion(&mut self, delta_time: f32) {
        self.motion_queue.advance_time(delta_time);

        // Read current part opacities from model for PartOpacity curve evaluation
        let mut motion_part_opacities: Vec<f32> = if let Some(ref model) = self.current_model {
            model.part_opacities()
        } else {
            Vec::new()
        };

        // Evaluate motion curves (parameters + part opacities)
        self.motion_queue.do_update_motion(
            &self.parameter_names,
            &self.param_lookup,
            &mut self.parameter_values,
            &self.eye_blink_param_ids,
            &self.lip_sync_param_ids,
            &self.part_ids,
            &mut motion_part_opacities,
        );

        // Write motion-updated part opacities to model
        if !motion_part_opacities.is_empty() {
            if let Some(ref mut model) = self.current_model {
                let mut opacities = model.part_opacities_mut();
                let dst = opacities.as_mut_slice();
                let len = dst.len().min(motion_part_opacities.len());
                dst[..len].copy_from_slice(&motion_part_opacities[..len]);
            }
        }

        // Apply expression (if active)
        self.expression_manager.apply(
            &self.parameter_names,
            &mut self.parameter_values,
            self.motion_queue.user_time_seconds,
        );

        // Apply EyeBlink controller — overrides motion for eye blink parameters
        if !self.eye_blink_param_ids.is_empty() {
            let blink = self.eye_blink.update(delta_time);
            if (blink - 1.0).abs() > 1e-6 {
                for id in &self.eye_blink_param_ids {
                    if let Some(idx) = self.parameter_names.iter().position(|n| n == id) {
                        self.parameter_values[idx] = blink;
                    }
                }
            }
        }

        // Apply Breath controller (delta-additive oscillation)
        self.breath.update(delta_time, &mut self.parameter_values, &self.parameter_names);

        // Apply Look controller (subtract old offset → update → add new offset)
        for p in &self.look.params {
            if let Some(idx) = self.parameter_names.iter().position(|n| n == &p.id) {
                if idx < self.parameter_values.len() {
                    self.parameter_values[idx] -= p.current_offset;
                }
            }
        }
        self.look.compute_raw(delta_time);
        for p in &self.look.params {
            if let Some(idx) = self.parameter_names.iter().position(|n| n == &p.id) {
                if idx < self.parameter_values.len() {
                    self.parameter_values[idx] += p.current_offset;
                }
            }
        }

        // Apply Physics (order 600 in CubismFramework: after Breath 500, before Pose 800)
        if let Some(ref mut engine) = self.physics {
            let mut params = motion::physics::PhysicsParams {
                values: &mut self.parameter_values,
                minimums: &self.parameter_mins,
                maximums: &self.parameter_maxs,
                defaults: &self.parameter_defaults,
                names: &self.parameter_names,
            };
            engine.evaluate(&mut params, delta_time);
        }

        // Auto-restart Idle when all motions have finished
        if self.auto_play_idle && self.motion_queue.entries.is_empty() {
            if let Some(idle_motions) = self.loaded_motions.get("idle") {
                if let Some(first) = idle_motions.first() {
                    self.motion_queue.start_motion(first.clone());
                }
            }
        }
    }

    /// Feed mouse NDC position into look controller for head/eye tracking.
    pub fn update_mouse_for_look(&mut self, mouse_x: f64, mouse_y: f64, screen_w: f32, screen_h: f32) {
        let ndc_x = 2.0 * mouse_x as f32 / screen_w - 1.0;
        let ndc_y = 1.0 - 2.0 * mouse_y as f32 / screen_h;
        self.look.set_target(ndc_x, ndc_y);
    }

    /// Handle tap interaction with camera values passed directly (avoids borrow conflict).
    #[allow(clippy::too_many_arguments)]
    pub fn handle_tap_with_cam(
        &mut self, x: f64, y: f64, screen_w: f32, screen_h: f32,
        cam_scale_x: f32, cam_scale_y: f32, cam_trans_x: f32, cam_trans_y: f32,
    ) {
        let ndc_x = 2.0 * x as f32 / screen_w - 1.0;
        let ndc_y = 1.0 - 2.0 * y as f32 / screen_h;
        let model_x = (ndc_x - cam_trans_x) / cam_scale_x;
        let model_y = (ndc_y - cam_trans_y) / cam_scale_y;

        // Hit test against current model drawables
        self.hit_test_drawables(model_x, model_y);
    }

    /// Check which hit area (if any) was tapped and play the associated motion.
    fn hit_test_drawables(&mut self, model_x: f32, model_y: f32) {
        let model = match self.current_model {
            Some(ref m) => m,
            None => return,
        };
        let drawable_ids: Vec<String> = match model {
            LoadedModelVariant::Core(m) => m.drawables().ids().iter()
                .map(|id| id.to_string_lossy().into_owned())
                .collect(),
            LoadedModelVariant::V2(a) => a.data.drawables.iter()
                .map(|d| d.id.to_string())
                .collect(),
        };

        // We need vertex data for hit testing. Collect frame drawables for this.
        // Use a temporary FrameDrawables to access vertex data.
        let model_mut_workaround = self.current_model.as_mut().unwrap(); // safe: we checked
        let fd = model_mut_workaround.collect_drawables();

        for hit_area in &self.hit_areas {
            let di = match drawable_ids.iter().position(|id| *id == hit_area.id) {
                Some(i) => i,
                None => continue,
            };
            if di >= fd.n { continue; }

            let vc = fd.vert_counts[di] as usize;
            let ic = fd.idx_counts[di] as usize;
            if vc < 3 || ic < 3 { continue; }

            let verts = unsafe { std::slice::from_raw_parts(fd.vert_positions[di], vc * 2) };
            let idx = unsafe { std::slice::from_raw_parts(fd.indices[di], ic) };

            for tri in idx.chunks(3) {
                if tri.len() < 3 { continue; }
                let ax = verts[tri[0] as usize * 2];
                let ay = verts[tri[0] as usize * 2 + 1];
                let bx = verts[tri[1] as usize * 2];
                let by = verts[tri[1] as usize * 2 + 1];
                let cx = verts[tri[2] as usize * 2];
                let cy = verts[tri[2] as usize * 2 + 1];

                if point_in_triangle(model_x, model_y, ax, ay, bx, by, cx, cy) {
                    if let Some(motions) = self.loaded_motions.get("tap_body") {
                        if !motions.is_empty() {
                            let idx = self.tap_count % motions.len();
                            self.tap_count += 1;
                            self.motion_queue.stop_all_motions();
                            let mut motion = motions[idx].clone();
                            motion.is_loop = false;
                            self.motion_queue.start_motion(motion);
                        }
                    }
                    return;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn point_in_triangle(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> bool {
    let d1 = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
    let d2 = (cx - bx) * (py - by) - (cy - by) * (px - bx);
    let d3 = (ax - cx) * (py - cy) - (ay - cy) * (px - cx);
    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(has_neg && has_pos)
}
