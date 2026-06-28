use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use super::safety::{is_command_allowed, sanitize_path, truncate_result, SafetyConfig};

const MAX_RESULT_CHARS: usize = 4096;

/// Execute a shell command and return its stdout + stderr.
///
/// Runs in a separate thread with a configurable timeout.
///
/// Arguments (from JSON):
///   cmd: string — the command to run.
///   timeout_secs: number (optional, default 5) — timeout in seconds.
pub fn exec_cmd(args: &Value, safety: &SafetyConfig) -> Result<String, String> {
    let cmd = args["cmd"]
        .as_str()
        .ok_or_else(|| "missing 'cmd' argument".to_string())?;
    let timeout = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(30);

    // Check if the command is in the allowed list
    if !is_command_allowed(cmd, &safety.allowed_commands) {
        return Err(format!(
            "command '{}' is not in the allowed commands list",
            cmd.split_whitespace().next().unwrap_or(cmd)
        ));
    }

    let cmd_owned = cmd.to_string();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd_owned)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let _ = tx.send(output);
    });

    let timeout_dur = Duration::from_secs(timeout);
    let output = match rx.recv_timeout(timeout_dur) {
        Ok(Ok(out)) => {
            let _ = handle.join();
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if !out.stderr.is_empty() {
                text.push_str(&format!(
                    "\nstderr:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Ok(truncate_result(&text, MAX_RESULT_CHARS))
        }
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(format!("command failed: {e}"))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Thread still running; we can't easily kill it, but return timeout
            Err(format!("command timed out after {timeout}s"))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("command thread crashed".to_string()),
    };

    output
}

/// Read a text file and return its contents.
///
/// Arguments (from JSON):
///   path: string — path to the file.
///   max_lines: number (optional) — maximum lines to read.
pub fn read_file(args: &Value, _safety: &SafetyConfig) -> Result<String, String> {
    let raw_path = args["path"]
        .as_str()
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let path = sanitize_path(raw_path, &_safety.allowed_read_paths)?;
    let max_lines = args.get("max_lines").and_then(|v| v.as_u64());

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read '{}': {e}", path.display()))?;

    let result = if let Some(max) = max_lines {
        let lines: Vec<&str> = content.lines().take(max as usize).collect();
        lines.join("\n")
    } else {
        content
    };

    Ok(truncate_result(&result, MAX_RESULT_CHARS))
}

/// List entries in a directory.
///
/// Arguments:
///   path: string — directory path.
///   max_entries: number (optional, default 50).
pub fn list_dir(args: &Value, _safety: &SafetyConfig) -> Result<String, String> {
    let raw_path = args["path"]
        .as_str()
        .ok_or_else(|| "missing 'path' argument".to_string())?;
    let path = sanitize_path(raw_path, &_safety.allowed_read_paths)?;
    let max_entries = args
        .get("max_entries")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;

    let entries: Vec<String> = std::fs::read_dir(&path)
        .map_err(|e| format!("cannot read directory '{}': {e}", path.display()))?
        .filter_map(|e| e.ok())
        .take(max_entries)
        .map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let kind = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                "📁"
            } else {
                "📄"
            };
            format!("{kind} {name}")
        })
        .collect();

    Ok(truncate_result(&entries.join("\n"), MAX_RESULT_CHARS))
}

/// Read /proc information.
///
/// Arguments:
///   pid: number (optional) — specific process ID.
///   stat: string (optional) — what to read ("status", "cmdline", "maps").
pub fn read_proc(args: &Value, _safety: &SafetyConfig) -> Result<String, String> {
    let pid = args
        .get("pid")
        .and_then(|v| v.as_u64())
        .map(|p| p.to_string())
        .unwrap_or_else(|| "self".to_string());
    let stat = args
        .get("stat")
        .and_then(|v| v.as_str())
        .unwrap_or("status");

    let path = format!("/proc/{pid}/{stat}");
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(truncate_result(&content, MAX_RESULT_CHARS)),
        Err(e) => Err(format!("cannot read {path}: {e}")),
    }
}

/// Get an environment variable value.
pub fn get_env(args: &Value, _safety: &SafetyConfig) -> Result<String, String> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| "missing 'name' argument".to_string())?;
    match std::env::var(name) {
        Ok(val) => Ok(val),
        Err(std::env::VarError::NotPresent) => Err(format!("env var '{name}' is not set")),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("env var '{name}' contains non-unicode data"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::tools::safety::SafetyConfig;

    fn safe_cfg() -> SafetyConfig {
        SafetyConfig {
            allowed_commands: vec![],
            allowed_read_paths: vec![],
            max_tool_rounds: 10,
        }
    }

    #[test]
    fn exec_cmd_echo() {
        let args = serde_json::json!({"cmd": "echo hello world"});
        let result = exec_cmd(&args, &safe_cfg()).unwrap();
        assert!(result.contains("hello world"));
    }

    #[test]
    fn exec_cmd_timeout() {
        let args = serde_json::json!({"cmd": "sleep 10", "timeout_secs": 1});
        let result = exec_cmd(&args, &safe_cfg());
        assert!(result.is_err());
    }

    #[test]
    fn read_file_etc_hostname() {
        let args = serde_json::json!({"path": "/etc/hostname"});
        assert!(read_file(&args, &safe_cfg()).is_ok());
    }

    #[test]
    fn read_file_nonexistent_returns_error() {
        let args = serde_json::json!({"path": "/nonexistent_file_xyz"});
        assert!(read_file(&args, &safe_cfg()).is_err());
    }

    #[test]
    fn get_env_path_exists() {
        let args = serde_json::json!({"name": "PATH"});
        let result = get_env(&args, &safe_cfg()).unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn get_env_nonexistent() {
        let args = serde_json::json!({"name": "THIS_VAR_SHOULD_NOT_EXIST_XYZ"});
        assert!(get_env(&args, &safe_cfg()).is_err());
    }
}
