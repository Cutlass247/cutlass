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
    /// open decode engines, keyed by source path
    engines: Mutex<HashMap<String, cutlass_engine::MediaEngine>>,
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
        "path": info.path,
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

#[tauri::command]
fn trim_clip(
    id: String,
    start: f64,
    len: f64,
    src_in: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    project.trim_clip(&id, start, len, src_in).map_err(err_str)?;
    Ok(project.snapshot())
}

#[tauri::command]
fn remove_clip(
    id: String,
    ripple: bool,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    if ripple {
        project.remove_clip_ripple(&id).map_err(err_str)?;
    } else {
        project.remove_clip(&id).map_err(err_str)?;
    }
    Ok(project.snapshot())
}

/// Full-quality frame at source time `t`, via the in-process engine.
/// Used when the playhead settles: the preview snaps from the 480p scrub
/// proxy to a real decoded frame. Frame-accurate, so on long-GOP sources
/// this can take a GOP of decode — the frontend calls it debounced.
#[tauri::command]
fn exact_frame(path: String, t: f64, state: State<AppState>) -> Result<String, String> {
    let mut engines = state.engines.lock().unwrap();
    let engine = match engines.entry(path.clone()) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => {
            v.insert(cutlass_engine::MediaEngine::open(&path).map_err(err_str)?)
        }
    };
    let frame = engine.frame_at(t, 1920).map_err(err_str)?;

    // JPEG has no alpha channel and image's encoder rejects Rgba8
    let rgb: Vec<u8> = frame
        .data
        .chunks_exact(4)
        .flat_map(|px| [px[0], px[1], px[2]])
        .collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
        .encode(&rgb, frame.width, frame.height, image::ExtendedColorType::Rgb8)
        .map_err(err_str)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            import_media,
            get_project,
            move_clip,
            trim_clip,
            remove_clip,
            exact_frame
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}
