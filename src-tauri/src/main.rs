#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn parse_json_payload(raw: &str) -> Result<Value, serde_json::Error> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return Ok(value);
    }

    if let Some(start) = raw.find('{') {
        return serde_json::from_str::<Value>(&raw[start..]);
    }

    serde_json::from_str::<Value>(raw)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunRequest {
    request: String,
    tone3000_api_key: Option<String>,
    gemini_api_key: Option<String>,
    output_dir: Option<String>,
    max_tones: Option<u8>,
    max_results: Option<u8>,
}

#[tauri::command]
fn run_download(payload: RunRequest) -> Result<Value, String> {
    if payload.request.trim().is_empty() {
        return Err("Request text is required.".to_string());
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .ok_or_else(|| "Failed to locate repository root.".to_string())?
        .to_path_buf();
    let script_path = repo_root.join("scripts").join("tauri_bridge.py");
    if !script_path.exists() {
        return Err(format!(
            "Bridge script not found: {}",
            script_path.display()
        ));
    }

    let output_dir = payload
        .output_dir
        .unwrap_or_else(|| "./smart_downloaded_tones".to_string());
    let max_tones = payload.max_tones.unwrap_or(3).clamp(1, 5).to_string();
    let max_results = payload.max_results.unwrap_or(15).clamp(5, 25).to_string();

    let mut base_args = vec![
        script_path.to_string_lossy().to_string(),
        "--request".to_string(),
        payload.request,
        "--output-dir".to_string(),
        output_dir,
        "--max-tones".to_string(),
        max_tones,
        "--max-results".to_string(),
        max_results,
    ];

    if let Some(key) = payload.tone3000_api_key.filter(|k| !k.trim().is_empty()) {
        base_args.push("--tone3000-key".to_string());
        base_args.push(key);
    }

    if let Some(key) = payload.gemini_api_key.filter(|k| !k.trim().is_empty()) {
        base_args.push("--gemini-key".to_string());
        base_args.push(key);
    }

    let mut command_errors: Vec<String> = Vec::new();
    for candidate in ["python3", "python"] {
        let output = Command::new(candidate)
            .current_dir(&repo_root)
            .args(base_args.iter())
            .output();

        let output = match output {
            Ok(out) => out,
            Err(err) => {
                command_errors.push(format!("{candidate}: {err}"));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            command_errors.push(format!(
                "{candidate}: empty response (exit: {}) {}",
                output.status, stderr
            ));
            continue;
        }

        match parse_json_payload(&stdout) {
            Ok(value) => return Ok(value),
            Err(err) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                command_errors.push(format!(
                    "{candidate}: invalid JSON ({err}). stderr: {stderr}. stdout: {stdout}"
                ));
            }
        }
    }

    Err(format!(
        "Failed to run Python bridge. {}",
        command_errors.join(" | ")
    ))
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![run_download])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
