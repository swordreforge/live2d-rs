use live2d_core::{Moc, Model};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crate::ai::types::CharacterCard;
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
pub fn find_v2_model_json(dir: &Path) -> Option<PathBuf> {
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

fn fallback_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Returns true if the model name looks like an engineering code
/// (e.g. "c455_01", "12345", "ev_cg001_02s") rather than a descriptive name.
fn is_generic_name(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    // Pure digits
    if chars.iter().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // No letters at all (digits+underscores only)
    if chars.iter().all(|c| c.is_ascii_digit() || *c == '_') {
        return true;
    }
    // Pattern: letter(s) + digits + _ + digits (e.g. "c455_01", "st01_rw")
    if let Some(pos) = name.find('_') {
        let before = &name[..pos];
        let after = &name[pos + 1..];
        if !before.is_empty()
            && !after.is_empty()
            && before.len() <= 5
            && before.chars().next().is_some_and(|c| c.is_alphabetic())
            && before.chars().skip(1).all(|c| c.is_alphanumeric())
            && after.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return true;
        }
    }
    false
}

/// Validate a specific model3.json file within a directory.
fn validate_model_dir_from_file(model3_path: &Path, base_dir: &Path) -> Result<(), String> {
    let model3 = crate::model_loader::Model3Json::from_file(model3_path)
        .map_err(|e| format!("parse model3.json: {e}"))?;
    let moc_path = base_dir.join(&model3.file_references.moc);
    if !moc_path.exists() {
        return Err(format!("moc3 not found: {}", moc_path.display()));
    }
    Ok(())
}

fn validate_model_dir(dir: &Path, format: ModelFormat) -> Result<(), String> {
    match format {
        ModelFormat::V3 => {
            let model3_path = crate::model_loader::find_model3_json(dir)
                .map_err(|e| format!("find model3.json: {e}"))?;
            let model3 = crate::model_loader::Model3Json::from_file(&model3_path)
                .map_err(|e| format!("parse model3.json: {e}"))?;
            let base = model3_path.parent().unwrap_or(dir);
            let moc_path = base.join(&model3.file_references.moc);
            if !moc_path.exists() {
                return Err(format!("moc3 not found: {}", moc_path.display()));
            }
            Ok(())
        }
        ModelFormat::V2 => {
            let model_json =
                find_v2_model_json(dir).ok_or_else(|| "no V2 model JSON".to_string())?;
            let json_text = std::fs::read_to_string(&model_json)
                .map_err(|e| format!("read model.json: {e}"))?;
            serde_json::from_str::<serde_json::Value>(&json_text)
                .map_err(|e| format!("invalid JSON: {e}"))?;
            Ok(())
        }
    }
}

pub struct ModelEntry {
    pub name: String,
    pub dir: PathBuf,
    pub loaded: bool,
    pub format: Option<ModelFormat>,
    /// Specific model3.json filename (e.g. "akira_st01_rw.model3.json")
    /// when a directory contains multiple model variants.
    pub model3_file: Option<String>,
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

/// Detect whether running on GNOME.
///
/// GNOME does not always set `XDG_CURRENT_DESKTOP`, so we check multiple
/// indicators in order of reliability, matching `check-wm.sh`:
///  1. `XDG_CURRENT_DESKTOP` contains "gnome"
///  2. `GNOME_DESKTOP_SESSION_ID` is set (legacy but reliable)
///  3. `gnome-shell` process is running via `pgrep`
pub fn is_gnome() -> bool {
    #[allow(clippy::if_same_then_else)]
    if std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_lowercase().contains("gnome"))
        .unwrap_or(false)
    {
        return true;
    }
    if std::env::var("GNOME_DESKTOP_SESSION_ID").is_ok() {
        return true;
    }
    std::process::Command::new("pgrep")
        .arg("-x")
        .arg("gnome-shell")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Detect whether running on KDE Plasma.
///
/// KDE reliably sets `XDG_CURRENT_DESKTOP`, so the env-var check suffices.
/// Falls back to `plasmashell` process check as a safety net.
pub fn is_kde() -> bool {
    if std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| {
            let d = d.to_lowercase();
            d.contains("kde")
        })
        .unwrap_or(false)
    {
        return true;
    }
    std::process::Command::new("pgrep")
        .arg("-x")
        .arg("plasmashell")
        .output()
        .map(|o| o.status.success())
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
    // Motion system — per-group queues so independent groups play concurrently
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
    /// UserData lookup from userdata3.json: drawable/part ID → description.
    pub user_data_map: HashMap<String, String>,
    /// Last hit UserData description text (displayed in GUI after tap).
    pub last_tapped_user_data: Option<String>,
    tap_count: usize,
    pub pose_data: Option<crate::model_loader::PoseData>,
    pub pose_fade_remaining: f32,
    /// Part IDs from the current model (for PartOpacity motion curves)
    pub part_ids: Vec<String>,
    /// Desktop pet mode: Off, Windowed, or AlwaysOnTop
    pub pet_mode: PetMode,
    /// Window click-through mode (input passthrough), toggled from tray menu.
    pub click_through: bool,
    /// Layout adjustment mode — shows pan/zoom sliders; saved to DB per model.
    pub layout_mode: bool,
    /// Set to true when pet_mode toggles so main.rs applies window changes
    pub pet_mode_changed: bool,
    /// True when camera needs recalculation (after pet mode window resize)
    pub camera_needs_fit: bool,
    /// Frame delay before showing pet toolbar (let window resize settle)
    pub pet_mode_delay: u32,
    /// True when pet mode needs window resize (after model switch)
    pub pet_resize_pending: bool,
    /// Frames to skip is_minimized() detection after a restore
    pub restore_cooldown: u32,
    /// Request minimize to floating circle
    pub request_minimize: bool,
    /// Request restore from floating circle
    pub request_restore: bool,
    /// True when window is minimized to a floating overlay
    pub minimized_to_float: bool,
    /// Frame counter for throttling Wayland pet sync when minimized
    /// (sync every 4th frame → ~15 Hz instead of 60 Hz, reducing alloc churn)
    pub pet_sync_counter: u8,
    /// Saved pet mode window size (logical pixels) for restore
    pub saved_window_pet_size: (f64, f64),
    /// Pre-built parameter name → index lookup (built once at model load)
    pub param_lookup: HashMap<String, usize>,
    /// Pre-built part ID → index lookup (built once at model load)
    pub part_lookup: HashMap<String, usize>,

    /// Scratch buffer for pet thread events, avoiding per-frame Vec allocation.
    #[cfg(target_os = "linux")]
    pub pet_events_scratch: Vec<crate::wayland_pet::PetEvent>,
    #[cfg(not(target_os = "linux"))]
    pub pet_events_scratch: Vec<()>,
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
    /// Last hovered V2 hit area name (tracked for per-area hover motion)
    pub v2_last_hovered_area: Option<String>,
    /// V2 motion sound lookup: group -> Vec<(mtn_filename, Option<absolute_sound_path>)>
    pub v2_motion_sounds: HashMap<String, Vec<(String, Option<PathBuf>)>>,
    /// Audio player instance
    pub audio_player: Option<crate::audio::AudioPlayer>,
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
    /// Parameter presets for current model
    pub preset_list: Vec<String>,
    /// Text buffer for naming a new preset
    pub preset_name_input: String,
    /// Search tab state
    pub search_query: String,
    pub search_results: Vec<db::SearchResult>,
    /// Pet toolbar search popup visibility
    pub pet_search_open: bool,
    pub scan_dirs: Vec<PathBuf>,
    pub settings_open: bool,
    pub scan_result: String,
    // ── AI Chat ──
    pub ai_enabled: bool,
    pub ai_messages: Vec<crate::ai::types::ChatMessage>,
    pub ai_state: crate::ai::types::AiState,
    pub ai_error: Option<String>,
    pub ai_config: crate::ai::types::AiConfig,
    pub ai_input_buffer: String,
    pub ai_chat_open: bool,
    pub ai_settings_open: bool,
    pub ai_test_result: Option<(String, bool)>,
    pub ai_result_rx: Option<std::sync::mpsc::Receiver<crate::ai::types::AiStreamEvent>>,
    pub tool_registry: crate::ai::tools::registry::ToolRegistry,
    pub safety_config: crate::ai::tools::safety::SafetyConfig,
    /// Counter for tool calling rounds in the current conversation turn.
    pub tool_round_counter: u32,
    /// Tools approved for the entire session (skip approval dialog).
    pub session_approved_tools: std::collections::HashSet<String>,
    /// Queue of dangerous tool calls awaiting user approval (from same batch).
    pub pending_tool_queue: Vec<crate::ai::types::ToolCall>,
    /// Raw text input buffer for tool-calling allowed commands setting.
    pub tool_calling_cmds_input: String,
    /// Raw text input buffer for tool-calling allowed paths setting.
    pub tool_calling_paths_input: String,
    /// Previous frame's settings panel visibility (for detecting open transition).
    pub settings_panel_was_open: bool,
    /// Timestamp (user_time_seconds) until which the current AI emotion persists.
    pub ai_emotion_until: Option<f64>,
    // ── TTS ──
    /// Cached voice list from the TTS API.
    pub tts_voices_cache: Vec<crate::ai::tts::TtsVoice>,
    /// Receiver for completed TTS audio file paths.
    pub tts_result_rx: Option<std::sync::mpsc::Receiver<std::path::PathBuf>>,
    /// Set to true to trigger a TTS voice list refresh on the next frame.
    pub tts_refresh_requested: bool,
    /// Per-model character cards cache (keyed by model file_path).
    pub character_cards: HashMap<String, CharacterCard>,
    /// Character card editor window visibility.
    pub character_card_editor_open: bool,

    // ── Screen Capture Preview ──
    /// Latest captured frame from the capture thread (feature: capture).
    #[cfg(feature = "capture")]
    pub capture_latest_frame: Option<crate::capture::CapturedFrame>,
    /// Egui texture handle for the capture preview.
    #[cfg(feature = "capture")]
    pub capture_texture: Option<egui::TextureHandle>,
    /// Whether the capture preview window is visible.
    #[cfg(feature = "capture")]
    pub capture_window_open: bool,
    /// Frame counter to detect new frames (avoids redundant texture uploads).
    #[cfg(feature = "capture")]
    pub capture_frame_count: u64,
    /// Active capture session (None = not capturing).
    #[cfg(feature = "capture")]
    pub(crate) capture_session: Option<crate::capture::CaptureSession>,
    /// Receiver for captured frames from the capture thread.
    #[cfg(feature = "capture")]
    pub(crate) capture_rx: Option<std::sync::mpsc::Receiver<crate::capture::CapturedFrame>>,
    /// Last time a vision auto-look was triggered.
    #[cfg(feature = "capture")]
    pub(crate) vision_last_look: Option<std::time::Instant>,
}

impl AppState {
    pub fn new(db: Option<db::AppDb>) -> Self {
        let ai_config = crate::ai::config::load_config(db.as_ref());
        let safety_config = crate::ai::tools::safety::SafetyConfig {
            allowed_commands: ai_config.allowed_commands.clone(),
            allowed_read_paths: ai_config.allowed_read_paths.clone(),
            max_tool_rounds: ai_config.max_tool_rounds,
            user_approved: false,
            working_dir: None,
        };
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
            user_data_map: HashMap::new(),
            last_tapped_user_data: None,
            tap_count: 0,
            pose_data: None,
            pose_fade_remaining: 0.0,
            part_ids: Vec::new(),
            pet_mode: PetMode::Off,
            click_through: false,
            layout_mode: false,
            pet_mode_changed: false,
            camera_needs_fit: false,
            pet_mode_delay: 0,
            pet_resize_pending: false,
            restore_cooldown: 0,
            request_minimize: false,
            request_restore: false,
            minimized_to_float: false,
            pet_sync_counter: 0,
            saved_window_pet_size: (0.0, 0.0),
            param_lookup: HashMap::new(),
            part_lookup: HashMap::new(),
            camera: Camera::new(),
            window_size: (800.0, 600.0),
            canvas_pixel_size: (0.0, 0.0),
            physics: None,
            v2_scale: 1.0,
            v2_last_hovered_area: None,
            last_v2_size: (0, 0),
            v2_motion_sounds: HashMap::new(),
            audio_player: crate::audio::AudioPlayer::new().ok(),
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
            #[cfg(target_os = "linux")]
            pet_events_scratch: Vec::new(),
            #[cfg(not(target_os = "linux"))]
            pet_events_scratch: Vec::new(),
            preset_list: Vec::new(),
            preset_name_input: String::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            pet_search_open: false,
            scan_dirs: Vec::new(),
            settings_open: false,
            scan_result: String::new(),
            ai_enabled: true,
            ai_messages: Vec::new(),
            ai_state: crate::ai::types::AiState::Idle,
            ai_error: None,
            ai_config,
            ai_input_buffer: String::new(),
            ai_chat_open: false,
            ai_settings_open: false,
            ai_test_result: None,
            ai_result_rx: None,
            tool_registry: crate::ai::tools::registry::ToolRegistry::builtin(),
            safety_config,
            tool_round_counter: 0,
            session_approved_tools: std::collections::HashSet::new(),
            pending_tool_queue: Vec::new(),
            tool_calling_cmds_input: String::new(),
            tool_calling_paths_input: String::new(),
            settings_panel_was_open: false,
            ai_emotion_until: None,
            tts_voices_cache: Vec::new(),
            tts_result_rx: None,
            tts_refresh_requested: false,
            character_cards: HashMap::new(),
            character_card_editor_open: false,

            #[cfg(feature = "capture")]
            capture_latest_frame: None,
            #[cfg(feature = "capture")]
            capture_texture: None,
            #[cfg(feature = "capture")]
            capture_window_open: true,
            #[cfg(feature = "capture")]
            capture_frame_count: 0,
            #[cfg(feature = "capture")]
            capture_session: None,
            #[cfg(feature = "capture")]
            capture_rx: None,
            #[cfg(feature = "capture")]
            vision_last_look: None,
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
        self.add_model_dir_inner(path, None);
    }

    fn add_model_dir_inner(&mut self, path: PathBuf, model3_file: Option<String>) {
        let format = detect_model_format(&path);
        let name = match (&format, &model3_file) {
            (Some(ModelFormat::V3), Some(f)) => f.trim_end_matches(".model3.json").to_string(),
            (Some(ModelFormat::V3), None) => {
                if let Ok(entries) = std::fs::read_dir(&path) {
                    entries
                        .flatten()
                        .find_map(|e| {
                            let n = e.file_name().to_string_lossy().to_string();
                            if n.ends_with(".model3.json") {
                                Some(n.trim_end_matches(".model3.json").to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| fallback_name(&path))
                } else {
                    fallback_name(&path)
                }
            }
            _ => fallback_name(&path),
        };
        let dir_string = path.to_string_lossy().to_string();
        let name_for_db = name.clone();
        self.model_list.push(ModelEntry {
            name,
            dir: path,
            loaded: false,
            format,
            model3_file,
        });
        if let Some(ref db) = self.db {
            let model_version = match format {
                Some(ModelFormat::V3) => "V3",
                Some(ModelFormat::V2) => "V2",
                None => "Unknown",
            };
            let _ = db.add_or_update_model(&dir_string, &name_for_db, model_version, None);
        }
    }

    /// Scan all configured directories recursively for V2/V3 models.
    /// Returns (added, skipped, invalid) counts.
    pub fn scan_and_add_models(&mut self) -> (usize, usize, usize) {
        let dirs: Vec<PathBuf> = self.scan_dirs.clone();
        let mut added = 0;
        let mut skipped = 0;
        let mut invalid = 0;
        for scan_dir in &dirs {
            let mut to_visit: Vec<PathBuf> = vec![scan_dir.clone()];
            while let Some(dir) = to_visit.pop() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    let mut model3_files: Vec<String> = Vec::new();
                    let mut has_v2 = false;
                    let mut subdirs: Vec<PathBuf> = Vec::new();

                    for entry in entries.flatten() {
                        let path = entry.path();
                        let fname = entry.file_name().to_string_lossy().to_string();
                        if path.is_dir() {
                            subdirs.push(path);
                        } else if fname.ends_with(".model3.json") {
                            model3_files.push(fname);
                        } else if fname.ends_with(".json") && is_v2_model_json(&fname) {
                            has_v2 = true;
                        }
                    }

                    // V3: one entry per .model3.json file
                    if !model3_files.is_empty() {
                        for mf in &model3_files {
                            let name = mf.trim_end_matches(".model3.json");
                            // Skip generic engineering code names
                            if is_generic_name(name) {
                                log::debug!("[scan] skipping generic name: {name}");
                                skipped += 1;
                                continue;
                            }
                            let fp = dir.join(mf);
                            let dir_str = dir.to_string_lossy().to_string();
                            let key = format!("{dir_str}|{mf}");
                            let already = self.model_list.iter().any(|e| {
                                let existing_key = format!(
                                    "{}|{}",
                                    e.dir.to_string_lossy(),
                                    e.model3_file.as_deref().unwrap_or("")
                                );
                                existing_key == key
                            });
                            if already {
                                skipped += 1;
                            } else if let Err(e) = validate_model_dir_from_file(&fp, &dir) {
                                log::warn!("[scan] {dir_str}/{mf}: {e}");
                                invalid += 1;
                            } else {
                                self.add_model_dir_inner(dir.clone(), Some(mf.clone()));
                                added += 1;
                            }
                        }
                    } else if has_v2 {
                        // V2: one entry per directory (V2 has no sub-model concept)
                        let dir_str = dir.to_string_lossy().to_string();
                        let already = self.model_list.iter().any(|e| e.dir == dir)
                            || self
                                .db
                                .as_ref()
                                .is_some_and(|d| d.get_model(&dir_str).ok().flatten().is_some());
                        if already {
                            skipped += 1;
                        } else if let Err(e) = validate_model_dir(&dir, ModelFormat::V2) {
                            log::warn!("[scan] {dir_str}: {e}");
                            invalid += 1;
                        } else {
                            self.add_model_dir_inner(dir.clone(), None);
                            added += 1;
                        }
                    }

                    to_visit.extend(subdirs);
                }
            }
        }
        (added, skipped, invalid)
    }

    /// Load scan directories from DB into scan_dirs field.
    pub fn load_scan_dirs(&mut self) {
        if let Some(ref db) = self.db {
            self.scan_dirs = db.get_scan_dirs().into_iter().map(PathBuf::from).collect();
        }
    }

    /// Save scan directories to DB.
    pub fn save_scan_dirs(&self) {
        if let Some(ref db) = self.db {
            let dirs: Vec<String> = self
                .scan_dirs
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            let _ = db.set_scan_dirs(&dirs);
        }
    }

    pub fn save_zoom(&mut self) {
        self.save_layout();
    }

    // ──────── Parameter presets ────────

    /// Refresh the preset list from the database for the current model.
    pub fn refresh_presets(&mut self) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => {
                self.preset_list.clear();
                return;
            }
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => {
                self.preset_list.clear();
                return;
            }
        };
        self.preset_list = match self.db {
            Some(ref db) => db.list_presets(&path).unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// Save current parameter values as a named preset for the current model.
    pub fn save_preset(&mut self, name: &str) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        if name.is_empty() {
            return;
        }
        // Serialize parameter names + values as JSON
        let pairs: Vec<(&str, f32)> = self
            .parameter_names
            .iter()
            .zip(self.parameter_values.iter())
            .map(|(n, v)| (n.as_str(), *v))
            .collect();
        let bytes = serde_json::to_vec(&pairs).unwrap_or_default();
        if bytes.is_empty() {
            return;
        }
        if let Some(ref db) = self.db {
            let _ = db.save_preset(&path, name, &bytes);
        }
        self.refresh_presets();
    }

    /// Load a named preset for the current model.
    pub fn load_preset(&mut self, name: &str) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        let bytes = match self.db {
            Some(ref db) => match db.load_preset(&path, name) {
                Ok(Some(b)) => b,
                _ => return,
            },
            None => return,
        };
        let pairs: Vec<(String, f32)> = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(_) => return,
        };
        // Overwrite parameter_values by matching names
        for (name, val) in &pairs {
            if let Some(&i) = self.param_lookup.get(name) {
                if i < self.parameter_values.len() {
                    self.parameter_values[i] = *val;
                }
            }
        }
        self.update_parameters();

        // Also apply to V2 model
        if self.is_v2 {
            if let Some(ref mut v2) = self.v2_model {
                for (name, val) in &pairs {
                    v2.set_param_value(name, *val, 1.0);
                }
            }
        }
    }

    /// Delete a named preset for the current model.
    pub fn delete_preset(&mut self, name: &str) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        if let Some(ref db) = self.db {
            let _ = db.delete_preset(&path, name);
        }
        self.refresh_presets();
    }

    // ──────── Layout persistence ────────

    /// Save current camera (pan + zoom) to DB for the current model.
    pub fn save_layout(&mut self) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        let (pan_x, pan_y, zoom) = if self.is_v2 {
            (None, None, Some(self.v2_scale))
        } else {
            let z = (self.camera.scale_x.abs() + self.camera.scale_y.abs()) / 2.0;
            (
                Some(self.camera.translate_x),
                Some(self.camera.translate_y),
                Some(z),
            )
        };
        if let Some(ref db) = self.db {
            let _ = db.set_model_layout(&path, pan_x, pan_y, zoom);
        }
    }

    /// Restore saved layout (pan + zoom) from DB for the current model.
    pub fn restore_layout(&mut self) {
        let idx = match self.current_idx {
            Some(i) => i,
            None => return,
        };
        let path = match self.model_list.get(idx) {
            Some(e) => e.dir.to_string_lossy().to_string(),
            None => return,
        };
        let rec = match self.db {
            Some(ref db) => match db.get_model(&path) {
                Ok(Some(r)) => r,
                _ => return,
            },
            None => return,
        };
        if self.is_v2 {
            if let Some(z) = rec.zoom_scale {
                self.v2_scale = z;
                if let Some(ref mut v2) = self.v2_model {
                    v2.set_scale(z);
                }
            }
        } else {
            let z = rec.zoom_scale.unwrap_or(1.0);
            self.camera.scale_x = self.camera.scale_x.signum() * z;
            self.camera.scale_y = self.camera.scale_y.signum() * z;
            if let Some(x) = rec.layout_pan_x {
                self.camera.translate_x = x;
            }
            if let Some(y) = rec.layout_pan_y {
                self.camera.translate_y = y;
            }
        }
    }

    /// Load the character card for the current model from DB and cache it.
    /// Returns the card (or a default if none exists).
    pub fn load_character_card(&mut self, file_path: &str) -> CharacterCard {
        let card = self
            .db
            .as_ref()
            .and_then(|db| db.get_character_card(file_path).ok())
            .flatten()
            .unwrap_or_default();
        self.character_cards
            .insert(file_path.to_string(), card.clone());
        card
    }

    /// Send a user message to the AI on a background thread.
    /// The result is picked up by `poll_ai_result()` on the next frame.
    pub fn send_ai_message(&mut self) {
        let text = self.ai_input_buffer.trim().to_string();
        if text.is_empty() || self.ai_state != crate::ai::types::AiState::Idle {
            return;
        }
        self.ai_input_buffer.clear();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        self.ai_messages.push(crate::ai::types::ChatMessage {
            role: crate::ai::types::ChatRole::User,
            content: text,
            timestamp,
            tool_call_id: None,
            tool_calls: None,
        });
        self.tool_round_counter = 0;

        let mut api_messages = Vec::new();

        // Build system prompt from character card fields + global system prompt
        let current_path = self
            .current_model_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let card = self
            .character_cards
            .get(&current_path)
            .cloned()
            .or_else(|| {
                self.db
                    .as_ref()
                    .and_then(|db| db.get_character_card(&current_path).ok())
                    .flatten()
            })
            .unwrap_or_default();

        let card_sections: Vec<String> = [
            ("角色名", &card.name),
            ("描述", &card.description),
            ("性格", &card.personality),
            ("场景", &card.scenario),
            ("对话示例", &card.example_dialogs),
            ("角色提示词", &card.system_prompt),
        ]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(label, value)| format!("【{label}】\n{value}"))
        .collect();

        let system_prompt = if !card_sections.is_empty() {
            let joined = card_sections.join("\n\n");
            if !self.ai_config.system_prompt.is_empty() {
                format!("{joined}\n\n---\n\n{}", self.ai_config.system_prompt)
            } else {
                joined
            }
        } else {
            self.ai_config.system_prompt.clone()
        };

        if !system_prompt.is_empty() {
            api_messages.push(crate::ai::types::ChatMessage {
                role: crate::ai::types::ChatRole::System,
                content: system_prompt,
                timestamp: 0.0,
                tool_call_id: None,
                tool_calls: None,
            });
        }
        // Inject relevant conversation memories as system context
        if self.ai_config.memory_enabled {
            if let Some(ref db) = self.db {
                let query = self
                    .ai_messages
                    .last()
                    .map(|m| m.content.as_str())
                    .unwrap_or("");
                if let Ok(memories) = db.search_memories(&current_path, query, 3) {
                    for mem in &memories {
                        api_messages.push(crate::ai::types::ChatMessage {
                            role: crate::ai::types::ChatRole::System,
                            content: format!("[相关记忆: {}]\n{}", mem.entry_type, mem.content),
                            timestamp: 0.0,
                            tool_call_id: None,
                            tool_calls: None,
                        });
                    }
                }
            }
        }

        let start = self
            .ai_messages
            .len()
            .saturating_sub(self.ai_config.context_length);
        api_messages.extend_from_slice(&self.ai_messages[start..]);

        self.ai_state = crate::ai::types::AiState::Waiting;
        self.ai_error = None;

        // Insert a placeholder assistant message that will be filled as tokens arrive.
        self.ai_messages.push(crate::ai::types::ChatMessage {
            role: crate::ai::types::ChatRole::Assistant,
            content: String::new(),
            timestamp,
            tool_call_id: None,
            tool_calls: None,
        });

        let config = self.ai_config.clone();
        let tools_defs = if config.tool_calling_enabled {
            Some(self.tool_registry.definitions())
        } else {
            None
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_result_rx = Some(rx);

        std::thread::spawn(move || {
            let client = crate::ai::client::AiChatClient::new();
            client.send_stream(&api_messages, &config, tools_defs.as_deref(), tx);
        });
    }

    /// Poll for the AI response from the background thread.
    /// Call once per frame from the UI loop.
    pub fn poll_ai_result(&mut self) {
        use crate::ai::types::{AiState, AiStreamEvent, ToolCall};

        // ── PendingTool state: wait for approval (handled by approve_tool/reject_tool) ──
        if matches!(self.ai_state, AiState::PendingTool { .. }) {
            return;
        }

        let rx = match self.ai_result_rx.take() {
            Some(rx) => rx,
            None => return,
        };

        let mut done = false;
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                AiStreamEvent::Token(t) => {
                    if let Some(last) = self.ai_messages.last_mut() {
                        last.content.push_str(&t);
                    }
                }
                AiStreamEvent::ToolCall(tc) => {
                    tool_calls.push(tc);
                }
                AiStreamEvent::Done => {
                    done = true;
                }
                AiStreamEvent::Error(e) => {
                    self.ai_error = Some(e);
                    done = true;
                    if let Some(last) = self.ai_messages.last() {
                        if last.role == crate::ai::types::ChatRole::Assistant
                            && last.content.is_empty()
                        {
                            self.ai_messages.pop();
                        }
                    }
                }
            }
        }

        if !done && tool_calls.is_empty() {
            // Still streaming — put the receiver back
            self.ai_result_rx = Some(rx);
            return;
        }

        // ── Handle tool calls ──
        if !tool_calls.is_empty() {
            // Pop the empty placeholder assistant message
            if let Some(last) = self.ai_messages.last() {
                if last.role == crate::ai::types::ChatRole::Assistant && last.content.is_empty() {
                    self.ai_messages.pop();
                }
            }
            // Push assistant message with tool_calls for display
            self.ai_messages.push(crate::ai::types::ChatMessage {
                role: crate::ai::types::ChatRole::Assistant,
                content: String::new(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                tool_call_id: None,
                tool_calls: Some(tool_calls.clone()),
            });

            // Split into safe/session-approved and dangerous tool calls
            let mut safe_tcs = Vec::new();
            let mut dangerous_tcs = Vec::new();
            for tc in tool_calls {
                let is_safe = self
                    .tool_registry
                    .safety_level(&tc.function.name)
                    .is_some_and(|l| matches!(l, crate::ai::tools::safety::SafetyLevel::Safe))
                    || self.session_approved_tools.contains(&tc.function.name);
                if is_safe {
                    safe_tcs.push(tc);
                } else {
                    dangerous_tcs.push(tc);
                }
            }

            // Auto-execute safe tools first
            if !safe_tcs.is_empty() {
                for tc in &safe_tcs {
                    if self.session_approved_tools.contains(&tc.function.name) {
                        self.safety_config.user_approved = true;
                    }
                }
                self.execute_tools_and_continue(safe_tcs);
                self.safety_config.user_approved = false;
            }

            // Queue dangerous tools and enter PendingTool for the first one
            if !dangerous_tcs.is_empty() {
                let first = dangerous_tcs.remove(0);
                self.pending_tool_queue = dangerous_tcs;
                match serde_json::from_str(&first.function.arguments) {
                    Ok(args) => {
                        self.ai_state = AiState::PendingTool {
                            tool_call_id: first.id,
                            tool_name: first.function.name,
                            args,
                        };
                    }
                    Err(_) => {
                        self.ai_state = AiState::Idle;
                        self.ai_error =
                            Some(format!("invalid tool args: {}", first.function.arguments));
                    }
                }
            }
            return;
        }

        // ── Normal completion (text response) ──
        self.ai_state = AiState::Idle;
        // Remove trailing empty assistant placeholder (no content arrived)
        if self.ai_messages.last().is_some_and(|m| {
            m.content.is_empty() && m.role == crate::ai::types::ChatRole::Assistant
        }) {
            self.ai_messages.pop();
        }
        // For tool-calling responses, the last message may be a Tool result.
        // Find the last Assistant message to extract emotion from.
        let last_assistant_idx = self.ai_messages.iter().rposition(|m| {
            m.role == crate::ai::types::ChatRole::Assistant && !m.content.is_empty()
        });
        let emotion = last_assistant_idx
            .and_then(|idx| {
                self.ai_messages
                    .get_mut(idx)
                    .and_then(|m| extract_emotion_tag(&mut m.content))
            });
        if let Some(emotion) = emotion {
            self.apply_emotion(&emotion);
        }
        // Save conversation to vector memory
        if self.ai_config.memory_enabled {
            let current_path = self
                .current_model_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(ref db) = self.db {
                let user_msg = self
                    .ai_messages
                    .iter()
                    .rev()
                    .skip(1)
                    .find(|m| m.role == crate::ai::types::ChatRole::User)
                    .map(|m| m.content.clone());
                let assistant_msg = self
                    .ai_messages
                    .last()
                    .filter(|m| m.role == crate::ai::types::ChatRole::Assistant)
                    .map(|m| m.content.clone());
                if let Some(ref msg) = user_msg {
                    let _ = db.save_memory(&current_path, msg, "message");
                }
                if let Some(ref msg) = assistant_msg {
                    if !msg.is_empty() {
                        let _ = db.save_memory(&current_path, msg, "message");
                    }
                }
            }
        }
        // Auto-speak the response via TTS
        if self.ai_config.tts_enabled && !self.ai_config.tts_key.is_empty() {
            let mut tts_config = self.ai_config.clone();
            let current_path = self
                .current_model_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(card) = self.character_cards.get(&current_path) {
                if !card.tts_voice.is_empty() {
                    tts_config.tts_voice = card.tts_voice.clone();
                }
            }
            if !tts_config.tts_voice.is_empty() {
                let text = self
                    .ai_messages
                    .last()
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                let text = crate::ai::tts::sanitize_text_for_tts(&text);
                if !text.is_empty() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.tts_result_rx = Some(rx);
                    std::thread::spawn(move || {
                        let result =
                            crate::ai::tts::synthesize(&tts_config, &text, &tts_config.tts_voice);
                        match result {
                            Ok(mp3_bytes) => {
                                let tmp_dir = std::env::temp_dir();
                                let path = tmp_dir.join(format!(
                                    "live2d-tts-{}.mp3",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_nanos()
                                ));
                                if std::fs::write(&path, &mp3_bytes).is_ok() {
                                    let _ = tx.send(path);
                                }
                            }
                            Err(e) => {
                                log::warn!("[tts] synthesis failed: {e}");
                            }
                        }
                    });
                }
            }
        } else {
            self.ai_result_rx = Some(rx);
        }
    }

    /// Map detected emotion to Live2D expression/parameters.
    fn apply_emotion(&mut self, emotion: &str) {
        use crate::motion::json::{ExpressionBlendMode, ParsedExp3Param, ParsedExpression};
        use crate::motion::ExpressionMotion;

        // Priority 1: use a loaded .exp3.json expression if it exists
        if self.loaded_expressions.contains_key(emotion) {
            if let Some(expr) = self.loaded_expressions.get(emotion).cloned() {
                self.expression_manager
                    .start_expression(expr, self.motion_queue.user_time_seconds);
                self.ai_emotion_until = Some(self.motion_queue.user_time_seconds as f64 + 5.0);
                return;
            }
        }

        // Priority 2: synthetic expression from built-in parameter map
        const EMOTION_MAP: &[(&str, &[(&str, f32)])] = &[
            (
                "happy",
                &[
                    ("ParamMouthForm", 0.5),
                    ("ParamMouthOpenY", 0.3),
                    ("ParamBrowLY", 0.3),
                    ("ParamBrowRY", 0.3),
                ],
            ),
            (
                "sad",
                &[
                    ("ParamMouthForm", -0.3),
                    ("ParamBrowLY", -0.3),
                    ("ParamBrowRY", -0.3),
                    ("ParamEyeLOpen", 0.5),
                    ("ParamEyeROpen", 0.5),
                ],
            ),
            (
                "angry",
                &[
                    ("ParamMouthForm", -0.5),
                    ("ParamBrowLY", -0.5),
                    ("ParamBrowRY", -0.5),
                    ("ParamBrowLX", -0.3),
                    ("ParamBrowRX", 0.3),
                ],
            ),
            (
                "surprised",
                &[
                    ("ParamMouthOpenY", 0.8),
                    ("ParamEyeLOpen", 1.0),
                    ("ParamEyeROpen", 1.0),
                    ("ParamBrowLY", 0.8),
                    ("ParamBrowRY", 0.8),
                ],
            ),
            (
                "neutral",
                &[
                    ("ParamMouthForm", 0.0),
                    ("ParamMouthOpenY", 0.0),
                    ("ParamEyeLOpen", 0.8),
                    ("ParamEyeROpen", 0.8),
                    ("ParamBrowLY", 0.0),
                    ("ParamBrowRY", 0.0),
                ],
            ),
            (
                "thinking",
                &[
                    ("ParamMouthForm", 0.2),
                    ("ParamEyeLOpen", 0.6),
                    ("ParamEyeROpen", 0.6),
                    ("ParamBrowLY", 0.3),
                    ("ParamBrowRY", 0.3),
                    ("ParamBrowLX", -0.2),
                    ("ParamBrowRX", 0.2),
                ],
            ),
            (
                "tired",
                &[
                    ("ParamMouthForm", -0.2),
                    ("ParamEyeLOpen", 0.4),
                    ("ParamEyeROpen", 0.4),
                    ("ParamBrowLY", -0.2),
                    ("ParamBrowRY", -0.2),
                ],
            ),
            (
                "satisfied",
                &[
                    ("ParamMouthForm", 0.3),
                    ("ParamMouthOpenY", 0.2),
                    ("ParamEyeLOpen", 0.7),
                    ("ParamEyeROpen", 0.7),
                    ("ParamBrowLY", 0.1),
                    ("ParamBrowRY", 0.1),
                ],
            ),
            (
                "embarrassed",
                &[
                    ("ParamMouthForm", 0.3),
                    ("ParamMouthOpenY", 0.1),
                    ("ParamEyeLOpen", 0.5),
                    ("ParamEyeROpen", 0.5),
                    ("ParamBrowLY", 0.1),
                    ("ParamBrowRY", 0.1),
                ],
            ),
        ];

        let params = EMOTION_MAP
            .iter()
            .find(|(name, _)| *name == emotion)
            .map(|(_, params)| params);

        if let Some(params) = params {
            let parsed = ParsedExpression {
                parameters: params
                    .iter()
                    .map(|(id, val)| ParsedExp3Param {
                        id: id.to_string(),
                        value: *val,
                        blend: ExpressionBlendMode::Override,
                    })
                    .collect(),
            };
            let expr = ExpressionMotion::new(parsed);
            self.expression_manager
                .start_expression(expr, self.motion_queue.user_time_seconds);
            self.ai_emotion_until = Some(self.motion_queue.user_time_seconds as f64 + 5.0);
        }
    }

    // ── Tool Calling: approval & execution ──

    /// Approve a pending tool call and execute it.
    pub fn approve_tool(&mut self) {
        use crate::ai::types::{AiState, ChatMessage, ChatRole};
        let state = std::mem::replace(&mut self.ai_state, AiState::Executing);
        match state {
            AiState::PendingTool {
                tool_call_id,
                tool_name,
                args,
            } => {
                // Enforce max tool rounds
                self.tool_round_counter += 1;
                if self.tool_round_counter > self.ai_config.max_tool_rounds {
                    self.ai_state = AiState::Idle;
                    self.ai_error = Some(format!(
                        "工具调用轮次已达上限 ({})，强制终止",
                        self.ai_config.max_tool_rounds
                    ));
                    return;
                }

                // Sync SafetyConfig from user-configured AiConfig
                self.safety_config.allowed_commands = self.ai_config.allowed_commands.clone();
                self.safety_config.max_tool_rounds = self.ai_config.max_tool_rounds;
                self.safety_config.user_approved = true;
                self.safety_config.working_dir = self.current_model_dir().map(|p| p.to_path_buf());

                let start = std::time::Instant::now();
                let result = self
                    .tool_registry
                    .execute(&tool_name, &args, &self.safety_config);
                let duration_ms = start.elapsed().as_millis() as u64;

                let content = match &result {
                    Ok(out) => out.clone(),
                    Err(e) => format!("Error: {e}"),
                };

                // Audit log
                let fp = self
                    .current_model_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Some(ref db) = self.db {
                    let args_json = serde_json::to_string(&args).unwrap_or_default();
                    let success = result.is_ok();
                    let _ = db.save_tool_execution(
                        &fp,
                        &tool_name,
                        &args_json,
                        &content,
                        success,
                        duration_ms,
                    );
                }

                self.ai_messages.push(ChatMessage {
                    role: ChatRole::Tool,
                    content,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0),
                    tool_call_id: Some(tool_call_id),
                    tool_calls: None,
                });
                self.safety_config.user_approved = false;
                self.process_pending_tool_queue();
            }
            _ => {
                self.ai_state = AiState::Idle;
            }
        }
    }

    /// Reject a pending tool call — tell the LLM it was denied.
    pub fn reject_tool(&mut self) {
        use crate::ai::types::{AiState, ChatMessage, ChatRole};
        let state = std::mem::replace(&mut self.ai_state, AiState::Idle);
        if let AiState::PendingTool {
            tool_call_id,
            tool_name,
            ref args,
        } = state
        {
            let fp = self
                .current_model_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(ref db) = self.db {
                let args_json = serde_json::to_string(args).unwrap_or_default();
                let _ = db.save_tool_execution(&fp, &tool_name, &args_json, "rejected", false, 0);
            }

            self.ai_messages.push(ChatMessage {
                role: ChatRole::Tool,
                content: "Operation rejected by user".to_string(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                tool_call_id: Some(tool_call_id),
                tool_calls: None,
            });
            self.process_pending_tool_queue();
        }
    }

    /// Process the next tool from the pending queue, or continue with the API.
    fn process_pending_tool_queue(&mut self) {
        if !self.pending_tool_queue.is_empty() {
            let tc = self.pending_tool_queue.remove(0);
            match serde_json::from_str(&tc.function.arguments) {
                Ok(args) => {
                    self.ai_state = crate::ai::types::AiState::PendingTool {
                        tool_call_id: tc.id,
                        tool_name: tc.function.name,
                        args,
                    };
                }
                Err(_) => {
                    self.ai_error =
                        Some(format!("invalid tool args: {}", tc.function.arguments));
                    self.continue_with_tool_result();
                }
            }
        } else {
            self.continue_with_tool_result();
        }
    }

    /// Spawn a non-streaming request to continue the multi-turn tool loop.
    fn continue_with_tool_result(&mut self) {
        use crate::ai::types::{AiState, AiStreamEvent, ChatMessage, ChatRole};

        let config = self.ai_config.clone();
        let tools_defs = if config.tool_calling_enabled {
            Some(self.tool_registry.definitions())
        } else {
            None
        };

        let start = self
            .ai_messages
            .len()
            .saturating_sub(self.ai_config.context_length);
        let api_messages: Vec<ChatMessage> = self.ai_messages[start..].to_vec();

        let current_path = self
            .current_model_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let card = self
            .character_cards
            .get(&current_path)
            .cloned()
            .or_else(|| {
                self.db
                    .as_ref()
                    .and_then(|db| db.get_character_card(&current_path).ok())
                    .flatten()
            })
            .unwrap_or_default();
        let card_sections: Vec<String> = [
            ("角色名", &card.name),
            ("描述", &card.description),
            ("性格", &card.personality),
            ("场景", &card.scenario),
            ("对话示例", &card.example_dialogs),
            ("角色提示词", &card.system_prompt),
        ]
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(label, value)| format!("【{label}】\n{value}"))
        .collect();
        let system_prompt = if !card_sections.is_empty() {
            let joined = card_sections.join("\n\n");
            if !config.system_prompt.is_empty() {
                format!("{joined}\n\n---\n\n{}", config.system_prompt)
            } else {
                joined
            }
        } else {
            config.system_prompt.clone()
        };

        let mut full_messages = Vec::new();
        if !system_prompt.is_empty() {
            full_messages.push(ChatMessage {
                role: ChatRole::System,
                content: system_prompt,
                timestamp: 0.0,
                tool_call_id: None,
                tool_calls: None,
            });
        }
        full_messages.extend(api_messages);

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_result_rx = Some(rx);
        self.ai_state = AiState::Waiting;

        // Push empty assistant placeholder so streaming tokens land in the right message
        self.ai_messages.push(ChatMessage {
            role: ChatRole::Assistant,
            content: String::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            tool_call_id: None,
            tool_calls: None,
        });

        std::thread::spawn(move || {
            let client = crate::ai::client::AiChatClient::new();
            if let Some(tools) = &tools_defs {
                // Non-streaming request with tools
                match client.send_with_tools(&full_messages, &config, tools) {
                    Ok(resp) => {
                        if let Some(content) = resp.content {
                            if !content.is_empty() {
                                // Send as a single token chunk
                                let _ = tx.send(AiStreamEvent::Token(content));
                            }
                        }
                        for tc in resp.tool_calls {
                            let _ = tx.send(AiStreamEvent::ToolCall(tc));
                        }
                        let _ = tx.send(AiStreamEvent::Done);
                    }
                    Err(e) => {
                        let _ = tx.send(AiStreamEvent::Error(e));
                    }
                }
            } else {
                // Fallback: streaming (no tools needed)
                client.send_stream(&full_messages, &config, None, tx);
            }
        });
    }

    /// Auto-execute a set of safe tool calls and continue.
    fn execute_tools_and_continue(&mut self, tool_calls: Vec<crate::ai::types::ToolCall>) {
        use crate::ai::types::{ChatMessage, ChatRole};

        // Enforce max tool rounds
        self.tool_round_counter += 1;
        if self.tool_round_counter > self.ai_config.max_tool_rounds {
            self.ai_state = crate::ai::types::AiState::Idle;
            self.ai_error = Some(format!(
                "工具调用轮次已达上限 ({})，强制终止",
                self.ai_config.max_tool_rounds
            ));
            return;
        }

        // Sync SafetyConfig from user-configured AiConfig
        self.safety_config.allowed_commands = self.ai_config.allowed_commands.clone();
        self.safety_config.max_tool_rounds = self.ai_config.max_tool_rounds;
        self.safety_config.working_dir = self.current_model_dir().map(|p| p.to_path_buf());

        let fp = self
            .current_model_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        for tc in tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&tc.function.arguments).unwrap_or_default();

            let start = std::time::Instant::now();
            let result = self
                .tool_registry
                .execute(&tc.function.name, &args, &self.safety_config);
            let duration_ms = start.elapsed().as_millis() as u64;

            let content = match &result {
                Ok(out) => out.clone(),
                Err(e) => format!("Error: {e}"),
            };

            // Audit log
            if let Some(ref db) = self.db {
                let args_json = serde_json::to_string(&args).unwrap_or_default();
                let success = result.is_ok();
                let _ = db.save_tool_execution(
                    &fp,
                    &tc.function.name,
                    &args_json,
                    &content,
                    success,
                    duration_ms,
                );
            }

            self.ai_messages.push(ChatMessage {
                role: ChatRole::Tool,
                content,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                tool_call_id: Some(tc.id),
                tool_calls: None,
            });
        }

        self.continue_with_tool_result();
    }

    /// Auto-reset expression back to neutral after the timeout elapses.
    pub fn tick_emotion_timeout(&mut self) {
        if let Some(until) = self.ai_emotion_until {
            let now = self.motion_queue.user_time_seconds as f64;
            if now >= until {
                self.expression_manager.clear();
                self.ai_emotion_until = None;
            }
        }
    }

    /// Fetch available TTS voices into the cache.
    pub fn refresh_tts_voices(&mut self) {
        let config = self.ai_config.clone();
        if config.tts_key.is_empty() {
            return;
        }
        match crate::ai::tts::list_voices(&config) {
            Ok(voices) => {
                self.tts_voices_cache = voices;
            }
            Err(e) => {
                log::warn!("[tts] failed to list voices: {e}");
            }
        }
    }

    /// Poll the TTS result channel for completed audio files and play them.
    pub fn poll_tts_result(&mut self) {
        let rx = match self.tts_result_rx.take() {
            Some(rx) => rx,
            None => return,
        };
        if let Ok(path) = rx.recv_timeout(std::time::Duration::from_millis(0)) {
            if let Some(ref player) = self.audio_player {
                player.play(&path);
            }
            // Clean up temp file after a short delay (best-effort)
            let path_clone = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                let _ = std::fs::remove_file(&path_clone);
            });
        } else {
            // Still pending — put the receiver back
            self.tts_result_rx = Some(rx);
        }
    }

    /// Start screen capture.  Spawns the capture thread and stores the receiver.
    #[cfg(feature = "capture")]
    pub fn start_capture(&mut self) {
        if self.capture_session.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        match crate::capture::CaptureSession::start(tx) {
            Ok(session) => {
                log::info!("Screen capture started");
                self.capture_session = Some(session);
                self.capture_rx = Some(rx);
            }
            Err(e) => log::error!("Failed to start capture: {e:#}"),
        }
    }

    /// Stop screen capture and clean up resources.
    #[cfg(feature = "capture")]
    pub fn stop_capture(&mut self) {
        self.capture_session.take();
        self.capture_rx.take();
        self.capture_latest_frame.take();
        self.capture_texture.take();
        log::info!("Screen capture stopped");
    }

    /// Returns true if capture is currently active.
    #[cfg(feature = "capture")]
    pub fn is_capturing(&self) -> bool {
        self.capture_session.is_some()
    }

    /// Called every frame to check if auto-look should fire.
    #[cfg(feature = "capture")]
    pub fn tick_vision_timer(&mut self) {
        if !self.ai_config.vision_auto_enabled {
            return;
        }
        if self.ai_state != crate::ai::types::AiState::Idle {
            return;
        }
        let interval = std::time::Duration::from_secs(
            self.ai_config.vision_interval_secs.max(10),
        );
        let should_fire = match self.vision_last_look {
            Some(t) => t.elapsed() >= interval,
            None => true,
        };
        if should_fire && self.capture_latest_frame.is_some() {
            self.vision_last_look = Some(std::time::Instant::now());
            self.trigger_vision_snapshot();
        }
    }

    /// Trigger a vision snapshot: encode the latest frame and send to AI.
    #[cfg(feature = "capture")]
    pub fn trigger_vision_snapshot(&mut self) {
        if self.ai_state != crate::ai::types::AiState::Idle {
            return;
        }

        let frame = match self.capture_latest_frame.take() {
            Some(f) => f,
            None => {
                log::warn!("No capture frame available for vision snapshot");
                return;
            }
        };
        let encoded = match crate::ai::vision::encode_frame(&frame) {
            Some(e) => e,
            None => {
                log::error!("Failed to encode frame for vision");
                return;
            }
        };

        self.ai_state = crate::ai::types::AiState::Waiting;

        let client = crate::ai::client::AiChatClient::new();
        let config = self.ai_config.clone();
        let messages: Vec<crate::ai::types::ChatMessage> = self
            .ai_messages
            .iter()
            .rev()
            .take(config.context_length)
            .cloned()
            .collect();
        let base64 = encoded.base64;
        let prompt = "Look at what's on the screen right now. Describe what you see in a short, conversational sentence.";

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            match client.send_vision(&messages, &base64, prompt, &config) {
                Ok(text) => {
                    let _ = tx.send(crate::ai::types::AiStreamEvent::Token(text));
                    let _ = tx.send(crate::ai::types::AiStreamEvent::Done);
                }
                Err(e) => {
                    let _ = tx.send(crate::ai::types::AiStreamEvent::Error(e));
                }
            }
        });
        self.ai_result_rx = Some(rx);

        self.ai_messages.push(crate::ai::types::ChatMessage {
            role: crate::ai::types::ChatRole::User,
            content: "[Looked at screen]".into(),
            timestamp,
            tool_call_id: None,
            tool_calls: None,
        });
    }

    /// Drain all pending captured frames into `capture_latest_frame`.
    #[cfg(feature = "capture")]
    pub fn drain_capture_frames(&mut self) {
        if let Some(ref rx) = self.capture_rx {
            while let Ok(frame) = rx.try_recv() {
                self.capture_frame_count += 1;
                self.capture_latest_frame = Some(frame);
            }
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
        let model3_file = entry.model3_file.clone();
        let _ = entry;
        self.clear_model_state();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            tx.send(load_v3_background(&dir, idx, model3_file.as_deref()))
                .ok();
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
        self.part_lookup.clear();
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
        self.part_lookup.clear();
        self.part_lookup.reserve(self.part_ids.len());
        for (i, id) in self.part_ids.iter().enumerate() {
            self.part_lookup.insert(id.clone(), i);
        }

        // Canvas info
        let canvas = model.canvas_info();
        self.canvas_pixel_size = (canvas.size_in_pixels.X, canvas.size_in_pixels.Y);

        self.current_moc = Some(moc);
        self.current_model = Some(model);
        self.current_idx = Some(idx);

        // Restore saved layout (pan + zoom) for this model
        self.restore_layout();

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

        // Refresh parameter presets for this model
        self.refresh_presets();

        // Start idle motion
        if self.auto_play_idle {
            self.try_start_idle_motion();
        }
    }

    /// Try to start the first idle motion.
    ///
    /// Some models (especially from games) store all motions under key `""`
    /// instead of `"Idle"`. Try `"Idle"` first, then fall back to `""`.
    fn try_start_idle_motion(&mut self) {
        // Clone the motion first to release the immutable borrow on loaded_motions
        let motion = self
            .loaded_motions
            .get("Idle")
            .or_else(|| self.loaded_motions.get(""))
            .and_then(|m| m.first().cloned());
        if let Some(m) = motion {
            self.motion_queue.start_motion(m, None);
        }
    }

    pub fn switch_to(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.model_list.len() {
            return Err("index out of range".into());
        }

        // Clear state
        self.current_model = None;
        self.current_moc = None;
        self.v2_model = None;
        self.last_v2_size = (0, 0); // force v2.resize() on next frame
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

            // Fill basic parameter names, values, ranges for GUI display
            let nparams = m.param_count();
            self.parameter_names.clear();
            self.parameter_mins.clear();
            self.parameter_maxs.clear();
            self.parameter_defaults.clear();
            self.param_lookup.clear();
            self.param_lookup.reserve(nparams as usize);
            self.parameter_values = Vec::with_capacity(nparams as usize);
            for i in 0..nparams {
                if let Ok(id) = m.param_id(i) {
                    self.param_lookup
                        .insert(id.clone(), self.parameter_names.len());
                    self.parameter_names.push(id);
                    self.parameter_values.push(m.param_value(i));
                    self.parameter_mins.push(m.param_min(i));
                    self.parameter_maxs.push(m.param_max(i));
                    self.parameter_defaults.push(m.param_default(i));
                }
            }

            let name = entry.name.clone();
            let _ = entry; // end borrow

            // Parse V2 model JSON hit_areas for per-part tap handling
            self.hit_areas.clear();
            if let Ok(json_text) = std::fs::read_to_string(&model_json) {
                #[derive(Deserialize)]
                struct V2HitArea {
                    name: String,
                    id: String,
                }
                #[derive(Deserialize)]
                struct V2ModelJson {
                    hit_areas: Option<Vec<V2HitArea>>,
                }
                if let Ok(parsed) = serde_json::from_str::<V2ModelJson>(&json_text) {
                    if let Some(areas) = parsed.hit_areas {
                        self.hit_areas = areas
                            .into_iter()
                            .map(|a| crate::model_loader::HitArea {
                                name: a.name,
                                id: a.id,
                            })
                            .collect();
                        log::info!("Loaded {} V2 hit areas", self.hit_areas.len());
                    }
                }
            }

            // Parse V2 model JSON motion sound paths
            if let Ok(json_text) = std::fs::read_to_string(&model_json) {
                let base_dir = model_json.parent().unwrap_or(&model_json).to_path_buf();
                self.v2_motion_sounds =
                    crate::v2_motion_sound::parse_v2_motions(&json_text, &base_dir);
                log::info!(
                    "Parsed {} V2 motion groups for sound",
                    self.v2_motion_sounds.len()
                );
            }

            self.v2_model = Some(m);
            self.current_idx = Some(idx);
            self.model_list[idx].loaded = true;
            log::info!("Loaded V2 model: {} (params={})", name, nparams);

            // Restore saved layout (zoom) for V2 model
            self.restore_layout();

            // Refresh parameter presets for this model
            self.refresh_presets();

            // Opening animation: play a random motion on model load
            if let Some(ref mut v2) = self.v2_model {
                v2.start_random_motion("", 3);
                self.play_v2_motion_sound();
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
            self.part_lookup.clear();
            self.part_lookup.reserve(self.part_ids.len());
            for (i, id) in self.part_ids.iter().enumerate() {
                self.part_lookup.insert(id.clone(), i);
            }

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
            self.load_user_data(&loaded.base_dir, &loaded.model3_json);

            // Start idle motion
            if self.auto_play_idle {
                self.try_start_idle_motion();
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

    /// Load userdata3.json if present and build the ID→description map.
    fn load_user_data(
        &mut self,
        base_dir: &std::path::Path,
        model3_json: &crate::model_loader::Model3Json,
    ) {
        let user_data_path = match model3_json.file_references.user_data {
            Some(ref p) => base_dir.join(p),
            None => return,
        };
        let json = match crate::model_loader::UserData3Json::from_file(&user_data_path) {
            Ok(j) => j,
            Err(e) => {
                log::warn!(
                    "Failed to load userdata3.json {}: {e}",
                    user_data_path.display()
                );
                return;
            }
        };
        self.user_data_map = json.to_map();
        log::info!(
            "Loaded userdata3.json ({} entries)",
            self.user_data_map.len()
        );
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
        let popac = parts.opacities_mut();

        for group in &pose.groups {
            let mut first_found = false;
            for entry in group {
                if let Some(&part_idx) = self.part_lookup.get(&entry.id) {
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
        let popac = parts.opacities_mut();

        // CopyPartOpacities: for any entry with links, copy main opacity to linked parts
        for group in &pose.groups {
            for entry in group {
                if entry.links.is_empty() {
                    continue;
                }
                if let Some(&main_idx) = self.part_lookup.get(&entry.id) {
                    let opacity = popac[main_idx];
                    for link_id in &entry.links {
                        if let Some(&link_idx) = self.part_lookup.get(link_id) {
                            popac[link_idx] = opacity;
                        }
                    }
                }
            }
        }
    }

    /// Start a motion from a specific category (e.g. "Idle", "TapBody").
    /// If `index` is provided, plays that specific motion; otherwise plays the first one.
    /// Falls back to `""` if the specific category is not found.
    ///
    /// Stack limits (single queue, total max 3 entries):
    /// - Idle motions: max 1 entry, infinite loops
    /// - Action motions (e.g. tap): max 2 entries, max 2 loops per entry
    pub fn start_motion(&mut self, category: &str, index: Option<usize>) -> bool {
        // V2 model: motions are internal to the C++ wrapper (loaded via model.json)
        if self.is_v2 {
            if let Some(ref mut v2) = self.v2_model {
                let idx = index.unwrap_or(0) as i32;
                v2.start_motion(category, idx, 3);
                self.play_v2_motion_sound();
            }
            return true;
        }

        let motion = {
            let motions = match self.loaded_motions.get(category) {
                Some(m) => m,
                None => match self.loaded_motions.get("") {
                    Some(m) => m,
                    None => return false,
                },
            };
            let idx = index.unwrap_or(0);
            if idx >= motions.len() {
                return false;
            }
            motions[idx].clone()
        };
        // Action motions (non-idle) get max 2 loops to prevent unbounded playback
        let is_idle = category == "Idle" || category.is_empty();
        let max_loops: Option<u32> = if is_idle { None } else { Some(2) };
        self.motion_queue.start_motion(motion, max_loops);
        true
    }

    pub fn update_parameters(&mut self) {
        if self.is_v2 {
            if let Some(ref mut v2) = self.v2_model {
                for (i, v) in self.parameter_values.iter().enumerate() {
                    if let Some(name) = self.parameter_names.get(i) {
                        v2.set_param_value(name, *v, 1.0);
                    }
                }
            }
        } else if let Some(ref mut model) = self.current_model {
            let mut params = model.parameters();
            let mut vals = params.values_mut();
            for (i, &v) in self.parameter_values.iter().enumerate() {
                vals.set(i, v);
            }
        }
    }

    /// Advance the motion system and apply to parameter values.
    /// Call this frame before `update_parameters`.
    ///
    /// Uses a single queue with mutual-exclusion (anti-conflict):
    /// when a new motion starts via `start_motion`, all existing entries
    /// begin fading out. This matches the official Cubism Framework behavior.
    pub fn advance_motion(&mut self, delta_time: f32) {
        self.motion_queue.advance_time(delta_time);

        // Evaluate motion curves from the single queue.
        // PartOpacity curves operate directly on model's part opacities in-place,
        // avoiding an extra Vec allocation and double copy (model → scratch → model)
        // of 50–200+ f32 values every frame.
        if let Some(ref mut model) = self.current_model {
            let mut parts = model.parts();
            let opacities = parts.opacities_mut();
            self.motion_queue.do_update_motion(
                &self.param_lookup,
                &mut self.parameter_values,
                &self.eye_blink_param_ids,
                &self.lip_sync_param_ids,
                &self.part_lookup,
                opacities,
            );
        } else {
            self.motion_queue.do_update_motion(
                &self.param_lookup,
                &mut self.parameter_values,
                &self.eye_blink_param_ids,
                &self.lip_sync_param_ids,
                &self.part_lookup,
                &mut [],
            );
        }

        // Apply expression (if active)
        self.expression_manager.apply(
            &self.param_lookup,
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

        // Auto-restart Idle when the queue becomes completely empty
        if self.auto_play_idle && self.motion_queue.entries.is_empty() {
            self.try_start_idle_motion();
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
            // Python convention: click triggers random motion from any group
            if let Some(ref mut v2) = self.v2_model {
                v2.start_random_motion("", 3);
                self.play_v2_motion_sound();
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

        // Determine which drawables to hit-test:
        // - Official models define HitAreas in model3.json → test only those drawables
        // - Non-official models often have no HitAreas → test all drawables as fallback
        let hit_drawable_indices: Vec<usize> = if self.hit_areas.is_empty() {
            // Fallback: test all drawables (whole-body tap)
            (0..drawable_ids.len()).collect()
        } else {
            self.hit_areas
                .iter()
                .filter_map(|area| {
                    drawable_ids
                        .iter()
                        .position(|id| id.to_string_lossy() == area.id)
                })
                .collect()
        };

        for di in hit_drawable_indices {
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
                    // Show UserData description for the hit drawable, if available
                    let drawable_id = drawable_ids[di].to_string_lossy();
                    self.last_tapped_user_data = self
                        .user_data_map
                        .get(drawable_id.as_ref())
                        .or_else(|| {
                            // Fallback: check part-level UserData via the drawable's parent part
                            let parent_idx = drawables.parent_part_indices()[di] as usize;
                            let part_ids: Vec<String> = model
                                .parts()
                                .ids()
                                .iter()
                                .map(|id| id.to_string_lossy().into_owned())
                                .collect();
                            let part_id = part_ids.get(parent_idx)?;
                            self.user_data_map.get(part_id.as_str())
                        })
                        .cloned();

                    // Try "TapBody" first, then fall back to "" (some models use flat keys)
                    let motions = self
                        .loaded_motions
                        .get("TapBody")
                        .or_else(|| self.loaded_motions.get(""));
                    if let Some(motions) = motions {
                        if !motions.is_empty() {
                            let idx = self.tap_count % motions.len();
                            self.tap_count += 1;
                            // Clone motion (owned) to release the immutable reference on loaded_motions
                            let mut motion = motions[idx].clone();
                            motion.is_loop = false;
                            self.motion_queue.start_motion(motion, Some(2));
                        }
                    }
                    return;
                }
            }
        }
    }

    /// After starting a V2 motion, look up the sound file from the parsed
    /// model.json motions and play it. Queries current_group/current_no from
    /// the C++ wrapper to know what was chosen (especially for random motions).
    fn play_v2_motion_sound(&mut self) {
        let group = self
            .v2_model
            .as_ref()
            .map(|v2| v2.current_group())
            .unwrap_or_default();
        let no = self
            .v2_model
            .as_ref()
            .map(|v2| v2.current_no() as usize)
            .unwrap_or(0);
        let Some(ref player) = self.audio_player else {
            return;
        };
        let Some(entries) = self.v2_motion_sounds.get(&group) else {
            return;
        };
        let Some((_file, Some(sound_path))) = entries.get(no) else {
            return;
        };
        player.play(sound_path);
    }

    /// Handle V2 hover: detect hit area transitions and play per-area motion.
    pub fn handle_v2_hover(&mut self, x: f64, y: f64) {
        if !self.is_v2 || self.hit_areas.is_empty() {
            return;
        }
        // Determine which hover area we're over (if any) — done in its own block
        // so the mutable v2 borrow is dropped before play_v2_motion_sound calls.
        let area_name: Option<String> = {
            let v2 = match self.v2_model.as_mut() {
                Some(v2) => v2,
                None => return,
            };
            let cx = x as f32;
            let cy = y as f32;
            let mut current: Option<String> = None;
            for area in &self.hit_areas {
                if v2.hit_test(&area.id, cx, cy) {
                    current = Some(area.name.clone());
                    break;
                }
            }
            current
        };

        if area_name != self.v2_last_hovered_area {
            if let Some(ref name) = area_name {
                if let Some(ref mut v2) = self.v2_model {
                    v2.start_random_motion(&format!("tap_{}", name), 3);
                }
                self.play_v2_motion_sound();
                if name == "head" {
                    if let Some(ref mut v2) = self.v2_model {
                        v2.start_random_motion("flick_head", 3);
                    }
                    self.play_v2_motion_sound();
                }
            }
            self.v2_last_hovered_area = area_name;
        }
    }
}

/// Scan the end of `content` for an emotion marker like `[happy]`.
/// If found, remove it from content and return the emotion name.
fn extract_emotion_tag(content: &mut String) -> Option<String> {
    let text = std::mem::take(content);
    let trimmed = text.trim();
    if let Some(start) = trimmed.rfind('[') {
        if let Some(end) = trimmed[start..].find(']') {
            let tag = &trimmed[start + 1..start + end];
            const VALID: &[&str] = &[
                "happy",
                "sad",
                "angry",
                "surprised",
                "neutral",
                "thinking",
                "tired",
                "satisfied",
                "embarrassed",
            ];
            if VALID.contains(&tag) {
                let before = &trimmed[..start];
                let after = &trimmed[start + end + 1..];
                *content = format!("{}{}", before.trim(), after.trim());
                return Some(tag.to_string());
            }
        }
    }
    *content = text;
    None
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
fn load_v3_background(
    dir: &Path,
    idx: usize,
    model3_file: Option<&str>,
) -> Result<V3RawData, String> {
    use crate::model_loader;

    let model3_path = if let Some(f) = model3_file {
        model_loader::find_model3_file(dir, f)
    } else {
        model_loader::find_model3_json(dir)
    }
    .map_err(|e| format!("find model3.json: {e}"))?;
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
