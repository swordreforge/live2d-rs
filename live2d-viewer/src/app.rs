use std::collections::HashMap;
use std::path::PathBuf;
use live2d_core::{Moc, Model};

use crate::motion;

pub struct ModelEntry {
    pub name: String,
    pub dir: PathBuf,
    pub loaded: bool,
}

pub struct AppState {
    pub model_list: Vec<ModelEntry>,
    pub current_idx: Option<usize>,
    pub current_moc: Option<Moc>,
    pub current_model: Option<Model<'static>>,
    pub parameter_values: Vec<f32>,
    pub parameter_names: Vec<String>,
    pub parameter_mins: Vec<f32>,
    pub parameter_maxs: Vec<f32>,
    pub texture_paths: Vec<PathBuf>,
    pub error_message: Option<String>,
    pub mouse_down: bool,
    pub last_mouse_x: f64,
    pub last_mouse_y: f64,
    // Motion system
    pub motion_queue: motion::MotionQueueManager,
    pub expression_manager: motion::ExpressionManager,
    /// Loaded motions by category (e.g. "Idle", "TapBody")
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
            texture_paths: Vec::new(),
            error_message: None,
            mouse_down: false,
            last_mouse_x: 0.0,
            last_mouse_y: 0.0,
            motion_queue: motion::MotionQueueManager::new(),
            expression_manager: motion::ExpressionManager::new(),
            loaded_motions: HashMap::new(),
            loaded_expressions: HashMap::new(),
            eye_blink_param_ids: Vec::new(),
            lip_sync_param_ids: Vec::new(),
            auto_play_idle: true,
            base_dir: None,
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

        // Drop order: Model first, then Moc
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
        self.motion_queue.stop_all_motions();

        let entry = &self.model_list[idx];
        let loaded = crate::model_loader::LoadedModel::load(&entry.dir)
            .map_err(|e| format!("load model: {e}"))?;

        let moc = Moc::revive(&loaded.moc3_data)
            .map_err(|e| format!("revive moc: {e}"))?;

        let moc_ptr: *const Moc = &moc as *const Moc;
        let model = unsafe { Model::initialize(&*moc_ptr) }
            .map_err(|e| format!("init model: {e}"))?;
        let model: Model<'static> = unsafe { std::mem::transmute(model) };

        let params = model.parameters();
        for id in params.ids() {
            self.parameter_names.push(id.to_string_lossy().into_owned());
        }
        self.parameter_values = params.default_values().to_vec();
        self.parameter_mins = params.minimum_values().to_vec();
        self.parameter_maxs = params.maximum_values().to_vec();

        self.current_moc = Some(moc);
        self.current_model = Some(model);
        self.current_idx = Some(idx);
        self.texture_paths = loaded.texture_paths();
        self.base_dir = Some(loaded.base_dir.clone());
        self.model_list[idx].loaded = true;

        // Extract eye blink and lip sync parameter IDs from model3.json Groups
        if let Some(ref groups) = loaded.model3_json.groups {
            for group in groups {
                match group.name.as_str() {
                    "EyeBlink" => {
                        self.eye_blink_param_ids = group.ids.clone();
                    }
                    "LipSync" => {
                        self.lip_sync_param_ids = group.ids.clone();
                    }
                    _ => {}
                }
            }
        }

        // Load all motion files
        self.load_all_motions(&loaded.base_dir, &loaded.model3_json);

        // Load all expression files
        self.load_all_expressions(&loaded.base_dir, &loaded.model3_json);

        // Start the first idle motion
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

    /// Start a motion from a specific category (e.g. "Idle", "TapBody").
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
            let mut params = model.parameters();
            let mut vals = params.values_mut();
            for (i, &v) in self.parameter_values.iter().enumerate() {
                vals.set(i, v);
            }
        }
    }

    /// Advance the motion system and apply to parameter values.
    /// Call this each frame before `update_parameters`.
    pub fn advance_motion(&mut self, delta_time: f32) {
        self.motion_queue.advance_time(delta_time);

        // Evaluate motion curves
        self.motion_queue.do_update_motion(
            &self.parameter_names,
            &mut self.parameter_values,
            &self.eye_blink_param_ids,
            &self.lip_sync_param_ids,
        );

        // Apply expression (if active)
        self.expression_manager.apply(
            &self.parameter_names,
            &mut self.parameter_values,
            self.motion_queue.user_time_seconds,
        );
    }
}
