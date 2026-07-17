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
    playback: Mutex<Option<cutlass_engine::player::PlaybackHandle>>,
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

/// Find the whisper model: CUTLASS_WHISPER_MODEL env var, or walk up
/// from the current dir looking for vendor/whisper/ggml-base.en.bin.
fn whisper_model_path() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("CUTLASS_WHISPER_MODEL") {
        return Ok(p.into());
    }
    let mut dir = std::env::current_dir().map_err(err_str)?;
    loop {
        let candidate = dir.join("vendor").join("whisper").join("ggml-base.en.bin");
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err("whisper model not found (vendor/whisper/ggml-base.en.bin)".into());
        }
    }
}

/// On-device transcription with word timestamps. Slow-ish (≈ 1/5 of the
/// clip duration on CPU); runs on a worker thread.
#[tauri::command]
fn transcribe_media(
    media_id: String,
    state: State<AppState>,
) -> Result<Vec<cutlass_engine::transcribe::Word>, String> {
    let path = state
        .media
        .lock()
        .unwrap()
        .get(&media_id)
        .map(|m| m.path.clone())
        .ok_or_else(|| format!("unknown media {media_id}"))?;
    let model = whisper_model_path()?;
    cutlass_engine::transcribe::transcribe(&path, &model.to_string_lossy()).map_err(err_str)
}

/// Delete a source range from a clip (the "delete these words" edit).
#[tauri::command]
fn razor_out(
    id: String,
    src_from: f64,
    src_to: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    project
        .razor_out(&id, src_from, src_to, &format!("c{nanos:x}"))
        .map_err(err_str)?;
    Ok(project.snapshot())
}

/// Start audio for the V1 track from `from_t`. Returns false (not an
/// error) when audio can't start — the UI then runs its silent local
/// clock, so playback still works on machines with no output device.
#[tauri::command]
fn play(from_t: f64, state: State<AppState>) -> bool {
    if let Some(h) = state.playback.lock().unwrap().take() {
        h.stop();
    }
    let clips: Vec<cutlass_engine::player::AudioClip> = {
        let project = state.project.lock().unwrap();
        let media = state.media.lock().unwrap();
        let snap = project.snapshot();
        snap["clips"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .filter(|c| c["track"] == "V1")
                    .filter_map(|c| {
                        let m = media.get(c["media"].as_str()?)?;
                        Some(cutlass_engine::player::AudioClip {
                            path: m.path.clone(),
                            start: c["start"].as_f64()?,
                            len: c["len"].as_f64()?,
                            src_in: c["src_in"].as_f64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    match cutlass_engine::player::start(clips, from_t) {
        Ok(handle) => {
            *state.playback.lock().unwrap() = Some(handle);
            true
        }
        Err(e) => {
            eprintln!("audio unavailable, playing silent: {e:#}");
            false
        }
    }
}

/// Stop audio; returns the timeline position where it stopped.
#[tauri::command]
fn pause(state: State<AppState>) -> Option<f64> {
    state.playback.lock().unwrap().take().map(|h| h.stop())
}

#[tauri::command]
fn playback_clock(state: State<AppState>) -> Option<serde_json::Value> {
    state
        .playback
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| json!({ "t": h.clock(), "ended": h.ended() }))
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
            exact_frame,
            play,
            pause,
            playback_clock,
            transcribe_media,
            razor_out
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}
