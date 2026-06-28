use std::path::PathBuf;

use crate::ai::types::AiConfig;

/// Directory: $CONFIG_DIR/live2d-viewer/
fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("live2d-viewer")
}

/// File path: $CONFIG_DIR/live2d-viewer/ai-config.json
fn config_path() -> PathBuf {
    config_dir().join("ai-config.json")
}

/// Load AI config from disk. Returns `Default` if the file doesn't exist or is corrupt.
pub fn load_config() -> AiConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
            log::warn!("Failed to parse AI config (using defaults): {e}");
            AiConfig::default()
        }),
        Err(_) => AiConfig::default(),
    }
}

/// Save AI config to disk atomically (write temp, then rename).
pub fn save_config(config: &AiConfig) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = config_path();
    let tmp = dir.join("ai-config.json.tmp");
    match serde_json::to_string_pretty(config) {
        Ok(json) => {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => log::error!("Failed to serialize AI config: {e}"),
    }
}
