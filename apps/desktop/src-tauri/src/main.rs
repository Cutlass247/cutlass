//! Cutlass desktop shell — walking skeleton.
//! Thin Tauri layer over cutlass-core: import media, mutate the CRDT
//! project, hand scrub-proxy frames to the UI as data URLs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use cutlass_core::media::{self, MediaInfo};
use cutlass_core::project::{Clip, Project};
use serde_json::json;
use tauri::State;

#[derive(Default)]
struct AppState {
    project: Mutex<Project>,
    media: Mutex<HashMap<String, MediaInfo>>,
}

fn err_str(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn data_url(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(err_str)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn media_json(info: &MediaInfo) -> Result<serde_json::Value, String> {
    let thumbs: Result<Vec<String>, String> =
        info.thumb_paths.iter().map(|p| data_url(p)).collect();
    Ok(json!({
        "id": info.id,
        "name": info.name,
        "duration_s": info.duration_s,
        "scrub_fps": info.scrub_fps,
        "thumbs": thumbs?,
    }))
}

/// Import a video: probe, build scrub proxy, append a clip to V1.
/// Runs on a worker thread (sync tauri command), so the UI stays live.
#[tauri::command]
fn import_media(path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    media::ensure_ffmpeg().map_err(err_str)?;
    let info = media::import(Path::new(&path)).map_err(err_str)?;
    let media_value = media_json(&info)?;

    let mut project = state.project.lock().unwrap();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let clip = Clip {
        id: format!("c{nanos:x}"),
        name: info.name.clone(),
        media: info.id.clone(),
        track: "V1".into(),
        start: project.track_end("V1"),
        len: info.duration_s,
        src_in: 0.0,
    };
    project.add_clip(&clip).map_err(err_str)?;
    state.media.lock().unwrap().insert(info.id.clone(), info);

    Ok(json!({ "media": media_value, "project": project.snapshot() }))
}

#[tauri::command]
fn get_project(state: State<AppState>) -> serde_json::Value {
    state.project.lock().unwrap().snapshot()
}

#[tauri::command]
fn move_clip(
    id: String,
    track: String,
    start: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    project.move_clip(&id, &track, start).map_err(err_str)?;
    Ok(project.snapshot())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![import_media, get_project, move_clip])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}
