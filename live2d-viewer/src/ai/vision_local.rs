use std::path::Path;
use std::process::Command;

pub fn infer_with_image(
    gguf_path: &str,
    jpeg_base64: &str,
    prompt: &str,
) -> Result<String, String> {
    let raw = decode_base64(jpeg_base64)?;
    let tmp = std::env::temp_dir().join("lv2_vis.jpg");
    std::fs::write(&tmp, &raw).map_err(|e| format!("write: {e}"))?;

    let result = run_vision_cli(gguf_path, &tmp, prompt);
    let _ = std::fs::remove_file(&tmp);
    result
}

fn decode_base64(b64: &str) -> Result<Vec<u8>, String> {
    let data = b64.strip_prefix("data:image/jpeg;base64,").unwrap_or(b64);
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("b64: {e}"))
}

fn run_vision_cli(gguf: &str, img: &Path, prompt: &str) -> Result<String, String> {
    let child = Command::new("llama-cli")
        .arg("-m").arg(gguf)
        .arg("--image").arg(img)
        .arg("-p").arg(prompt)
        .arg("--temp").arg("0.7")
        .arg("-n").arg("256")
        .arg("--no-display-prompt")
        .arg("--simple-io")
        .arg("--single-turn")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    match child {
        Ok(c) => {
            let out = c.wait_with_output().map_err(|e| format!("llama-cli: {e}"))?;
            if out.status.success() {
                return String::from_utf8(out.stdout)
                    .map(|s| s.trim().to_string())
                    .map_err(|e| format!("utf8: {e}"));
            }
        }
        Err(e) => return Err(format!("spawn llama-cli: {e}")),
    }

    Err("llama-cli failed".into())
}
