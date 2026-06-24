use live2d_core::{Moc, Model};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::camera::Camera;
use crate::db;
use crate::motion;

/// Determine whether a model directory contains a V2 or V3 model.
#[derive(Clone, Copy)]
pub enum ModelFormat {
    V2,
    V3,
}

/// Check if a filename looks like a V2 model JSON (e.g. model.json, model0.json, xxx.model.json)
fn is_v2_model_json(name: &str) -> bool {
    // V2 model JSON: contains "model" (case-insensitive) in the name, ends with .json,
    // and is NOT a V3 .model3.json
    let lower = name.to_lowercase();
    lower.contains("model") && name.ends_with(".json") && !name.ends_with(".model3.json")
}

/// Find the V2 model JSON file in a directory (e.g. model.json, model0.json).
fn find_v2_model_json(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_v2_model_json(&name) {
            return Some(entry.path());
        }
    }
    None
}

pub fn detect_model_format(dir: &Path) -> Option<ModelFormat> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".model3.json") {
            return Some(ModelFormat::V3);
        }
    }
    // No .model3.json found — check if there's a V2 model JSON
    find_v2_model_json(dir).map(|_| ModelFormat::V2)
}

pub struct ModelEntry {
    pub name: String,
    pub dir: PathBuf,
    pub loaded: bool,
    pub format: Option<ModelFormat>,
}

/// Raw V3 model data loaded from background thread (all I/O done off main thread).
pub struct V3RawData {
    pub idx: usize,
    pub moc3_bytes: Vec<u8>,
    pub base_dir: PathBuf,
    pub texture_paths: Vec<PathBuf>,
    #[allow(clippy::type_complexity)]
    pub motion_files: Vec<(String, Vec<(Vec<u8>, Option<f32>, Option<f32>)>)>,
    pub expression_files: Vec<(String, Vec<u8>)>,
    pub pose_bytes: Option<Vec<u8>>,
    pub physics_bytes: Option<Vec<u8>>,
    pub hit_areas_bytes: Option<Vec<u8>>,
    pub groups_bytes: Option<Vec<u8>>,
}

/// Pending async model switch state.
pub enum PendingLoad {
    None,
    V3Loading(mpsc::Receiver<Result<V3RawData, String>>),
}

/// Pet mode type — mutually exclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PetMode {
    Off,
    /// Transparent frameless window with click-through (legacy behavior).
    Windowed,
    /// Separate Wayland layer-shell surface always on top.
    AlwaysOnTop,
}

/// Detect whether running on GNOME (GNOME does not support wlr-layer-shell).
pub fn is_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_lowercase().contains("gnome"))
        .unwrap_or(false)
}

pub struct AppState {
    pub model_list: Vec<ModelEntry>,
    pub current_idx: Option<usize>,
    /// V2-specific model handle
    pub v2_model: Option<live2d_v2_core::Model>,
    /// True when current model is V2 format
    pub is_v2: bool,
    pub current_moc: Option<Moc>,
    pub current_model: Option<Model<'static>>,
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
    pub hit_areas: Vec<crate::model_loader::HitArea>,
    tap_count: usize,
    pub pose_data: Option<crate::model_loader::PoseData>,
    pub pose_fade_remaining: f32,
    /// Part IDs from the current model (for PartOpacity motion curves)
    pub part_ids: Vec<String>,
    /// Desktop pet mode: Off, Windowed, or AlwaysOnTop
    pub pet_mode: PetMode,
    /// Window click-through mode (input passthrough), toggled from tray menu.
    pub click_through: bool,
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
    /// V2 zoom scale factor (tracked here because MatrixManager has no getter)
    pub v2_scale: f32,
    /// Last V2 resize dimensions — skip v2.resize() in render loop if unchanged
    pub last_v2_size: (i32, i32),
    /// Pending async model switch (V3 loads files on background thread)
    pub pending_load: PendingLoad,
    /// Optional database for model history and settings persistence
    pub db: Option<db::AppDb>,
    /// Index of model currently being renamed (None = not renaming)
    pub renaming_idx: Option<usize>,
    /// Buffer for in-place rename text edit
    pub renaming_buffer: String,
    // ── Wayland pet mode thread (only on Linux) ──
    #[cfg(target_os = "linux")]
    pub pet_wayland_cmd_tx: Option<mpsc::Sender<crate::wayland_pet::PetCommand>>,
    #[cfg(target_os = "linux")]
    pub pet_wayland_event_rx: Option<mpsc::Receiver<crate::wayland_pet::PetEvent>>,
    #[cfg(target_os = "linux")]
    pub pet_wayland_thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(not(target_os = "linux"))]
    pub pet_wayland_cmd_tx: Option<()>,
    #[cfg(not(target_os = "linux"))]
    pub pet_wayland_event_rx: Option<()>,
    #[cfg(not(target_os = "linux"))]
    pub pet_wayland_thread: Option<()>,
}

impl AppState {
    pub fn new(db: Option<db::AppDb>) -> Self {
        Self {
            model_list: Vec::new(),
            current_idx: None,
            v2_model: None,
            is_v2: false,
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
            pet_mode: PetMode::Off,
            click_through: false,
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
            v2_scale: 1.0,
            last_v2_size: (0, 0),
            pending_load: PendingLoad::None,
            db,
            renaming_idx: None,
            renaming_buffer: String::new(),
            #[cfg(target_os = "linux")]
            pet_wayland_cmd_tx: None,
            #[cfg(target_os = "linux")]
            pet_wayland_event_rx: None,
            #[cfg(target_os = "linux")]
            pet_wayland_thread: None,
            #[cfg(not(target_os = "linux"))]
            pet_wayland_cmd_tx: None,
            #[cfg(not(target_os = "linux"))]
            pet_wayland_event_rx: None,
            #[cfg(not(target_os = "linux"))]
            pet_wayland_thread: None,
        }
    }

    pub fn current_model_dir(&self) -> Option<PathBuf> {
        self.current_idx
            .and_then(|i| self.model_list.get(i))
            .map(|e| e.dir.clone())
    }

    pub fn current_model_format(&self) -> Option<ModelFormat> {
        self.current_idx
            .and_then(|i| self.model_list.get(i))
            .and_then(|e| e.format)
    }

    pub fn add_model_dir(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let format = detect_model_format(&path);
        let dir_string = path.to_string_lossy().to_string();
        let name_for_db = name.clone();
        self.model_list.push(ModelEntry {
            name,
            dir: path,
            loaded: false,
            format,
        });
        // Record in database
        if let Some(ref db) = self.db {
            let model_version = match format {
                Some(ModelFormat::V3) => "V3",
                Some(ModelFormat::V2) => "V2",
                None => "Unknown",
            };
            let _ = db.add_or_update_model(&dir_string, &name_for_db, model_version, None);
        }
    }

    /// Save current zoom/scale for the active model to the database.
    pub fn save_zoom(&mut self) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        let zoom = if self.is_v2 {
            Some(self.v2_scale)
        } else {
            Some((self.camera.scale_x + self.camera.scale_y) / 2.0)
        };
        if let Some(ref db) = self.db {
            let _ = db.set_zoom(&path, zoom);
        }
    }

    /// Start switching to model at `idx` asynchronously.
    /// V3: spawns background thread for I/O, returns immediately.
    /// V2: falls back to synchronous switch (C++ does GL work internally).
    pub fn begin_switch(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.model_list.len() {
            return Err("index out of range".into());
        }
        let entry = &self.model_list[idx];
        let fmt = entry
            .format
            .ok_or_else(|| format!("no model file found in {:?}", entry.dir))?;

        self.is_v2 = matches!(fmt, ModelFormat::V2);

        if self.is_v2 {
            return self.switch_to(idx);
        }

        // V3: clear current model state immediately, spawn background thread
        let dir = entry.dir.clone();
        let _ = entry;
        self.clear_model_state();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            tx.send(load_v3_background(&dir, idx)).ok();
        });
        self.pending_load = PendingLoad::V3Loading(rx);
        log::info!("Started background load for idx={idx}");
        Ok(())
    }

    /// Poll the pending switch channel. If data arrived, process it.
    /// Returns true if a switch was completed (or errored).
    pub fn complete_pending_switch(&mut self) -> bool {
        let rx = match &mut self.pending_load {
            PendingLoad::V3Loading(rx) => rx,
            _ => return false,
        };
        let data = match rx.try_recv() {
            Ok(data) => data,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("Background load thread disconnected");
                self.pending_load = PendingLoad::None;
                return false;
            }
        };
        self.pending_load = PendingLoad::None;
        match data {
            Ok(raw) => {
                let idx = raw.idx;
                log::info!("Background load complete for idx={idx}, processing on main thread");
                self.complete_v3_switch(idx, raw);
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
        true
    }

    /// Clear all V3 model state (called before new load).
    fn clear_model_state(&mut self) {
        self.current_model = None;
        self.current_moc = None;
        self.v2_model = None;
        self.parameter_values.clear();
        self.parameter_names.clear();
        self.parameter_mins.clear();
        self.parameter_maxs.clear();
        self.parameter_defaults.clear();
        self.loaded_motions.clear();
        self.loaded_expressions.clear();
        self.eye_blink_param_ids.clear();
        self.lip_sync_param_ids.clear();
        self.hit_areas.clear();
        self.pose_data = None;
        self.physics = None;
        self.part_ids.clear();
        self.motion_queue.stop_all_motions();
    }

    /// Process V3 raw data on main thread (Moc::revive, Model::initialize, parse JSONs).
    fn complete_v3_switch(&mut self, idx: usize, raw: V3RawData) {
        use crate::model_loader;

        // Moc::revive + Model::initialize (GL texture uploads happen here)
        let moc = match Moc::revive(&raw.moc3_bytes) {
            Ok(m) => m,
            Err(e) => {
                self.error_message = Some(format!("revive moc: {e}"));
                return;
            }
        };
        let moc_ptr: *const Moc = &moc as *const Moc;
        let model = match unsafe { Model::initialize(&*moc_ptr) } {
            Ok(m) => m,
            Err(e) => {
                self.error_message = Some(format!("init model: {e}"));
                return;
            }
        };
        let model: Model<'static> = unsafe { std::mem::transmute(model) };

        // Parameters
        let params = model.parameters();
        for id in params.ids() {
            self.parameter_names.push(id.to_string_lossy().into_owned());
        }
        self.parameter_values = params.default_values().to_vec();
        self.parameter_mins = params.minimum_values().to_vec();
        self.parameter_maxs = params.maximum_values().to_vec();
        self.parameter_defaults = params.default_values().to_vec();

        self.param_lookup.clear();
        self.param_lookup.reserve(self.parameter_names.len());
        for (i, name) in self.parameter_names.iter().enumerate() {
            self.param_lookup.insert(name.clone(), i);
        }

        // Part IDs
        let parts = model.parts();
        self.part_ids = parts
            .ids()
            .iter()
            .map(|id| id.to_string_lossy().into_owned())
            .collect();

        // Canvas info
        let canvas = model.canvas_info();
        self.canvas_pixel_size = (canvas.size_in_pixels.X, canvas.size_in_pixels.Y);

        self.current_moc = Some(moc);
        self.current_model = Some(model);
        self.current_idx = Some(idx);

        // Restore saved zoom for this model
        if let Some(ref db) = self.db {
            let path = &self.model_list[idx].dir;
            if let Ok(Some(rec)) = db.get_model(&path.to_string_lossy()) {
                if let Some(z) = rec.zoom_scale {
                    self.camera.scale_x = z;
                    self.camera.scale_y = z;
                    self.camera.translate_x = 0.0;
                    self.camera.translate_y = 0.0;
                    log::info!("Restored zoom={:.2} for {}", z, rec.name);
                }
            }
        }

        self.texture_paths = raw.texture_paths;
        self.base_dir = Some(raw.base_dir);
        self.model_list[idx].loaded = true;
        self.is_v2 = false;

        // Groups (eye blink, lip sync)
        if let Some(ref gbytes) = raw.groups_bytes {
            if let Ok(groups) = serde_json::from_slice::<Vec<model_loader::Group>>(gbytes) {
                for group in &groups {
                    match group.name.as_str() {
                        "EyeBlink" => self.eye_blink_param_ids = group.ids.clone(),
                        "LipSync" => self.lip_sync_param_ids = group.ids.clone(),
                        _ => {}
                    }
                }
            }
        }

        // Hit areas
        if let Some(ref hbytes) = raw.hit_areas_bytes {
            if let Ok(areas) = serde_json::from_slice::<Vec<model_loader::HitArea>>(hbytes) {
                self.hit_areas = areas;
            }
        }

        // Parse motions from pre-loaded raw bytes
        for (category, entries) in &raw.motion_files {
            let mut motions = Vec::new();
            for (bytes, fade_in, fade_out) in entries {
                match motion::json::parse_motion_json(bytes) {
                    Ok(parsed) => {
                        let fi = fade_in.unwrap_or(-1.0);
                        let fo = fade_out.unwrap_or(-1.0);
                        motions.push(motion::CubismMotion::new(parsed, fi, fo));
                    }
                    Err(e) => log::warn!("Parse motion {category}: {e}"),
                }
            }
            self.loaded_motions.insert(category.clone(), motions);
        }

        // Parse expressions
        for (name, bytes) in &raw.expression_files {
            match motion::json::parse_expression_json(bytes) {
                Ok(parsed) => {
                    self.loaded_expressions
                        .insert(name.clone(), motion::ExpressionMotion::new(parsed));
                }
                Err(e) => log::warn!("Parse expression {name}: {e}"),
            }
        }

        // Pose
        if let Some(ref bytes) = raw.pose_bytes {
            match model_loader::parse_pose_json(bytes) {
                Ok(p) => {
                    log::info!("Loaded pose with {} groups", p.groups.len());
                    self.pose_data = Some(p);
                }
                Err(e) => log::warn!("Parse pose: {e}"),
            }
        }

        // Apply pose reset (needs current_model)
        if self.pose_data.is_some() {
            self.apply_pose_reset();
        }

        // Physics
        if let Some(ref bytes) = raw.physics_bytes {
            match motion::physics::PhysicsEngine::from_json(bytes) {
                Ok(mut engine) => {
                    log::info!("Loaded physics ({} sub-rigs)", engine.sub_rig_count());
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
                Err(e) => log::warn!("Parse physics: {e}"),
            }
        }

        // Start idle motion
        if self.auto_play_idle {
            if let Some(idle_motions) = self.loaded_motions.get("Idle") {
                if let Some(first) = idle_motions.first() {
                    self.motion_queue.start_motion(first.clone());
                    log::info!("Started idle motion");
                }
            }
        }
    }

    pub fn switch_to(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.model_list.len() {
            return Err("index out of range".into());
        }

        // Clear V3 state
        self.current_model = None;
        self.current_moc = None;
        self.v2_model = None;
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
        let fmt = entry
            .format
            .ok_or_else(|| format!("no model file found in {:?}", entry.dir))?;

        self.is_v2 = matches!(fmt, ModelFormat::V2);

        if self.is_v2 {
            // ── V2 model path ──
            // Find the model JSON (model.json, model0.json, xxx.model.json, etc.)
            let model_json = find_v2_model_json(&entry.dir)
                .ok_or_else(|| format!("no V2 model JSON found in {:?}", entry.dir))?;

            let mut m =
                live2d_v2_core::Model::new().map_err(|e| format!("create V2 model: {e}"))?;
            m.load_json(&model_json.to_string_lossy())
                .map_err(|e| format!("V2 load_json: {e}"))?;

            // Pre-fetch V2 canvas info for window sizing
            let cw = m.canvas_width();
            let ch = m.canvas_height();
            self.canvas_pixel_size = (cw, ch);

            // Fill basic parameter names for GUI display
            let nparams = m.param_count();
            self.parameter_names.clear();
            self.param_lookup.clear();
            self.param_lookup.reserve(nparams as usize);
            for i in 0..nparams {
                if let Ok(id) = m.param_id(i) {
                    self.param_lookup
                        .insert(id.clone(), self.parameter_names.len());
                    self.parameter_names.push(id);
                }
            }

            let name = entry.name.clone();
            let _ = entry; // end borrow
            self.v2_model = Some(m);
            self.current_idx = Some(idx);
            self.model_list[idx].loaded = true;
            log::info!("Loaded V2 model: {} (params={})", name, nparams);

            // Restore saved zoom for V2 model
            if let Some(ref db) = self.db {
                let p = self.model_list[idx].dir.to_string_lossy().to_string();
                if let Ok(Some(rec)) = db.get_model(&p) {
                    if let Some(z) = rec.zoom_scale {
                        self.v2_scale = z;
                        if let Some(ref mut v2) = self.v2_model {
                            v2.set_scale(z);
                        }
                        log::info!("Restored V2 zoom={:.2} for {}", z, rec.name);
                    }
                }
            }
        } else {
            // ── V3 model path (existing code) ──
            let loaded = crate::model_loader::LoadedModel::load(&entry.dir)
                .map_err(|e| format!("load model: {e}"))?;

            let moc = Moc::revive(&loaded.moc3_data).map_err(|e| format!("revive moc: {e}"))?;

            let moc_ptr: *const Moc = &moc as *const Moc;
            let model =
                unsafe { Model::initialize(&*moc_ptr) }.map_err(|e| format!("init model: {e}"))?;
            let model: Model<'static> = unsafe { std::mem::transmute(model) };

            let params = model.parameters();
            for id in params.ids() {
                self.parameter_names.push(id.to_string_lossy().into_owned());
            }
            self.parameter_values = params.default_values().to_vec();
            self.parameter_mins = params.minimum_values().to_vec();
            self.parameter_maxs = params.maximum_values().to_vec();
            self.parameter_defaults = params.default_values().to_vec();

            // Build param lookup once — reused every frame
            self.param_lookup.clear();
            self.param_lookup.reserve(self.parameter_names.len());
            for (i, name) in self.parameter_names.iter().enumerate() {
                self.param_lookup.insert(name.clone(), i);
            }

            // Read part IDs for PartOpacity motion curve evaluation
            let parts = model.parts();
            self.part_ids = parts
                .ids()
                .iter()
                .map(|id| id.to_string_lossy().into_owned())
                .collect();

            // Read canvas info
            let canvas = model.canvas_info();
            self.canvas_pixel_size = (canvas.size_in_pixels.X, canvas.size_in_pixels.Y);

            self.current_moc = Some(moc);
            self.current_model = Some(model);
            self.current_idx = Some(idx);
            self.texture_paths = loaded.texture_paths();
            self.base_dir = Some(loaded.base_dir.clone());
            self.model_list[idx].loaded = true;

            // Extract eye blink and lip sync parameter IDs
            if let Some(ref groups) = loaded.model3_json.groups {
                for group in groups {
                    match group.name.as_str() {
                        "EyeBlink" => self.eye_blink_param_ids = group.ids.clone(),
                        "LipSync" => self.lip_sync_param_ids = group.ids.clone(),
                        _ => {}
                    }
                }
            }

            // Load hit areas
            if let Some(ref areas) = loaded.model3_json.hit_areas {
                self.hit_areas = areas.clone();
            }

            // Load all motion/expression/pose/physics
            self.load_all_motions(&loaded.base_dir, &loaded.model3_json);
            self.load_all_expressions(&loaded.base_dir, &loaded.model3_json);
            self.load_pose(&loaded.base_dir, &loaded.model3_json);
            self.apply_pose_reset();
            self.load_physics(&loaded.base_dir, &loaded.model3_json);

            // Start idle motion
            if self.auto_play_idle {
                if let Some(idle_motions) = self.loaded_motions.get("Idle") {
                    if let Some(first) = idle_motions.first() {
                        self.motion_queue.start_motion(first.clone());
                        log::info!("Started idle motion");
                    }
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
        log::info!(
            "Loaded pose with {} groups (fade={:.2}s)",
            parsed.groups.len(),
            parsed.fade_in_time
        );
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
        let mut parts = model.parts();
        let pids: Vec<String> = parts
            .ids()
            .iter()
            .map(|id| id.to_string_lossy().into_owned())
            .collect();
        let popac = parts.opacities_mut();

        for group in &pose.groups {
            let mut first_found = false;
            for entry in group {
                if let Some(part_idx) = pids.iter().position(|id| id == &entry.id) {
                    if !first_found {
                        popac[part_idx] = 1.0;
                        first_found = true;
                    } else {
                        popac[part_idx] = 0.0;
                    }
                }
            }
        }
        // Propagate part opacities to drawables
        if let Some(ref mut model) = self.current_model {
            model.update();
        }
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
        let mut parts = model.parts();
        let pids: Vec<String> = parts
            .ids()
            .iter()
            .map(|id| id.to_string_lossy().into_owned())
            .collect();
        let popac = parts.opacities_mut();

        // CopyPartOpacities: for any entry with links, copy main opacity to linked parts
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

        // Read current part opacities from model for PartOpacity curve evaluation
        let mut motion_part_opacities: Vec<f32> = if let Some(ref model) = self.current_model {
            model.parts().opacities().to_vec()
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

        // Write motion-updated part opacities to model (pose system will override
        // specific pose-group parts in update_pose, which runs after this)
        if !motion_part_opacities.is_empty() {
            if let Some(ref mut model) = self.current_model {
                let mut parts = model.parts();
                let opacities = parts.opacities_mut();
                let len = opacities.len().min(motion_part_opacities.len());
                opacities[..len].copy_from_slice(&motion_part_opacities[..len]);
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
                    if let Some(&idx) = self.param_lookup.get(id) {
                        self.parameter_values[idx] = blink;
                    }
                }
            }
        }

        // Apply Breath controller (delta-additive oscillation)
        self.breath
            .update(delta_time, &mut self.parameter_values, &self.param_lookup);

        // Apply Look controller (subtract old offset → update → add new offset)
        for p in &self.look.params {
            if let Some(&idx) = self.param_lookup.get(&p.id) {
                if idx < self.parameter_values.len() {
                    self.parameter_values[idx] -= p.current_offset;
                }
            }
        }
        self.look.compute_raw(delta_time);
        for p in &self.look.params {
            if let Some(&idx) = self.param_lookup.get(&p.id) {
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
            if let Some(idle_motions) = self.loaded_motions.get("Idle") {
                if let Some(first) = idle_motions.first() {
                    self.motion_queue.start_motion(first.clone());
                }
            }
        }
    }

    /// Feed mouse NDC position into look controller for head/eye tracking.
    pub fn update_mouse_for_look(
        &mut self,
        mouse_x: f64,
        mouse_y: f64,
        screen_w: f32,
        screen_h: f32,
    ) {
        let ndc_x = 2.0 * mouse_x as f32 / screen_w - 1.0;
        let ndc_y = 1.0 - 2.0 * mouse_y as f32 / screen_h;
        self.look.set_target(ndc_x, ndc_y);
    }

    /// Handle tap interaction with camera values passed directly (avoids borrow conflict).
    /// V2 uses built-in hitTest() with raw screen coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_tap_with_cam(
        &mut self,
        x: f64,
        y: f64,
        screen_w: f32,
        screen_h: f32,
        cam_scale_x: f32,
        cam_scale_y: f32,
        cam_trans_x: f32,
        cam_trans_y: f32,
    ) {
        if self.is_v2 {
            // V2: hit test all body parts using full MVP transform (accounts for zoom/pan)
            let hit = self
                .v2_model
                .as_ref()
                .map(|v2| !v2.hit_part(x as f32, y as f32, false).is_empty())
                .unwrap_or(false);
            if hit {
                if let Some(ref mut v2) = self.v2_model {
                    v2.start_random_motion("TapBody", 3);
                }
            }
            return;
        }

        let model = match self.current_model {
            Some(ref m) => m,
            None => return,
        };

        let ndc_x = 2.0 * x as f32 / screen_w - 1.0;
        let ndc_y = 1.0 - 2.0 * y as f32 / screen_h;
        let model_x = (ndc_x - cam_trans_x) / cam_scale_x;
        let model_y = (ndc_y - cam_trans_y) / cam_scale_y;

        let drawables = model.drawables();
        let drawable_ids = drawables.ids();
        let vpos = drawables.vertex_positions();
        let vcounts = drawables.vertex_counts();
        let idxs = drawables.indices();
        let icounts = drawables.index_counts();

        for hit_area in &self.hit_areas {
            let di = match drawable_ids
                .iter()
                .position(|id| id.to_string_lossy() == hit_area.id)
            {
                Some(i) => i,
                None => continue,
            };

            let verts = unsafe { std::slice::from_raw_parts(vpos[di], vcounts[di] as usize) };
            let idx = unsafe { std::slice::from_raw_parts(idxs[di], icounts[di] as usize) };

            for tri in idx.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }
                let a = &verts[tri[0] as usize];
                let b = &verts[tri[1] as usize];
                let c = &verts[tri[2] as usize];

                if point_in_triangle(model_x, model_y, a.X, a.Y, b.X, b.Y, c.X, c.Y) {
                    if let Some(motions) = self.loaded_motions.get("TapBody") {
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
fn point_in_triangle(
    px: f32,
    py: f32,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    cx: f32,
    cy: f32,
) -> bool {
    let d1 = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
    let d2 = (cx - bx) * (py - by) - (cy - by) * (px - bx);
    let d3 = (ax - cx) * (py - cy) - (ay - cy) * (px - cx);
    let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(has_neg && has_pos)
}

/// Background thread: read all V3 model files from disk.
fn load_v3_background(dir: &Path, idx: usize) -> Result<V3RawData, String> {
    use crate::model_loader;

    let model3_path =
        model_loader::find_model3_json(dir).map_err(|e| format!("find model3.json: {e}"))?;
    let base_dir = model3_path.parent().unwrap_or(dir).to_path_buf();

    let json_str =
        std::fs::read_to_string(&model3_path).map_err(|e| format!("read model3.json: {e}"))?;
    let json: model_loader::Model3Json =
        serde_json::from_str(&json_str).map_err(|e| format!("parse model3.json: {e}"))?;

    let moc3_path = base_dir.join(&json.file_references.moc);
    let moc3_bytes = std::fs::read(&moc3_path).map_err(|e| format!("read moc3: {e}"))?;

    let texture_paths: Vec<PathBuf> = json
        .file_references
        .textures
        .iter()
        .map(|p| base_dir.join(p))
        .collect();

    let mut motion_files = Vec::new();
    if let Some(ref motions) = json.file_references.motions {
        for (category, refs) in motions {
            let mut entries = Vec::new();
            for mref in refs {
                let path = base_dir.join(&mref.file);
                match std::fs::read(&path) {
                    Ok(bytes) => entries.push((
                        bytes,
                        mref.fade_in_time.map(|f| f as f32),
                        mref.fade_out_time.map(|f| f as f32),
                    )),
                    Err(e) => log::warn!("read motion {}: {e}", mref.file),
                }
            }
            motion_files.push((category.clone(), entries));
        }
    }

    let mut expression_files = Vec::new();
    if let Some(ref exprs) = json.file_references.expressions {
        for eref in exprs {
            let path = base_dir.join(&eref.file);
            match std::fs::read(&path) {
                Ok(bytes) => expression_files.push((eref.name.clone(), bytes)),
                Err(e) => log::warn!("read expression {}: {e}", eref.file),
            }
        }
    }

    let pose_bytes = json
        .file_references
        .pose
        .as_ref()
        .and_then(|p| std::fs::read(base_dir.join(p)).ok());

    let physics_bytes = json
        .file_references
        .physics
        .as_ref()
        .and_then(|p| std::fs::read(base_dir.join(p)).ok());

    let hit_areas_bytes = serde_json::to_vec(&json.hit_areas).ok();
    let groups_bytes = serde_json::to_vec(&json.groups).ok();

    Ok(V3RawData {
        idx,
        moc3_bytes,
        base_dir,
        texture_paths,
        motion_files,
        expression_files,
        pose_bytes,
        physics_bytes,
        hit_areas_bytes,
        groups_bytes,
    })
}
