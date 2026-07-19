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

/// Undo entries are (before, after) clip states; restoring either side
/// applies forward CRDT changes, so history behaves under collab.
#[derive(Default)]
struct History {
    undo: Vec<(Vec<Clip>, Vec<Clip>)>,
    redo: Vec<(Vec<Clip>, Vec<Clip>)>,
}

#[derive(Default)]
struct AppState {
    project: Mutex<Project>,
    history: Mutex<History>,
    media: Mutex<HashMap<String, MediaInfo>>,
    /// open decode engines, keyed by source path
    engines: Mutex<HashMap<String, cutlass_engine::MediaEngine>>,
    playback: Mutex<Option<cutlass_engine::player::PlaybackHandle>>,
    /// pings the collab task after a local edit, when a session is live
    sync_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<SyncCmd>>>,
    room: Mutex<Option<String>>,
}

enum SyncCmd {
    /// local doc changed — push sync messages
    Ping,
    /// ephemeral presence payload to relay (playhead, name, color)
    Presence(String),
}

/// Every mutating command calls this so a live collab session pushes the
/// change out. No session → no-op.
fn notify_sync(state: &State<AppState>) {
    if let Some(tx) = state.sync_tx.lock().unwrap().as_ref() {
        let _ = tx.send(SyncCmd::Ping);
    }
}

/// Run a mutation with undo capture + sync notification.
fn with_undo(
    state: &State<AppState>,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    let before = project.clips_state();
    f(&mut project)?;
    let after = project.clips_state();
    let snap = project.snapshot();
    drop(project);
    if before != after {
        let mut h = state.history.lock().unwrap();
        h.undo.push((before, after));
        if h.undo.len() > 100 {
            h.undo.remove(0);
        }
        h.redo.clear();
    }
    notify_sync(state);
    Ok(snap)
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
        "waveform": info.waveform,
    }))
}

/// Engine-native import: probe, sample proxy frames in one sequential
/// decode pass, extract waveform peaks — all in-process, no ffmpeg CLI.
fn import_with_engine(path: &Path) -> anyhow::Result<MediaInfo> {
    let path_str = path.to_string_lossy().to_string();
    let mut eng = cutlass_engine::MediaEngine::open(&path_str)?;
    let duration_s = eng.duration_s();
    anyhow::ensure!(duration_s > 0.05, "no usable duration");
    let scrub_fps =
        (cutlass_core::media::MAX_SCRUB_FRAMES / duration_s.max(0.1)).min(10.0);
    let dir = cutlass_core::media::cache_dir(path)?;
    let mut thumb_paths = cutlass_core::media::read_frames(&dir)?;
    if thumb_paths.is_empty() {
        let mut i = 0u32;
        eng.sample_frames(1.0 / scrub_fps, cutlass_core::media::SCRUB_WIDTH, |f| {
            i += 1;
            let rgb: Vec<u8> = f.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut out = std::fs::File::create(dir.join(format!("f{i:05}.jpg")))?;
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode(
                &rgb,
                f.width,
                f.height,
                image::ExtendedColorType::Rgb8,
            )?;
            Ok(())
        })?;
        thumb_paths = cutlass_core::media::read_frames(&dir)?;
        anyhow::ensure!(!thumb_paths.is_empty(), "no frames sampled");
    }
    Ok(MediaInfo {
        id: format!("m{:016x}", cutlass_core::media::path_hash(path)),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "clip".into()),
        path: path_str.clone(),
        duration_s,
        scrub_fps,
        thumb_paths,
        waveform: cutlass_engine::audio::waveform_peaks(&path_str, 1200),
    })
}

/// Engine first; ffmpeg-CLI fallback for containers libav chokes on.
fn import_any(path: &Path) -> anyhow::Result<MediaInfo> {
    import_with_engine(path).or_else(|e| {
        eprintln!("engine import failed ({e:#}); falling back to ffmpeg CLI");
        media::ensure_ffmpeg()?;
        media::import(path)
    })
}

/// Import a video: probe, build scrub proxy, append a clip to V1.
/// Runs on a worker thread (sync tauri command), so the UI stays live.
#[tauri::command]
fn import_media(path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let info = import_any(Path::new(&path)).map_err(err_str)?;
    let media_value = media_json(&info)?;

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let snap = with_undo(&state, |project| {
        let clip = Clip {
            id: format!("c{nanos:x}"),
            name: info.name.clone(),
            media: info.id.clone(),
            track: "V1".into(),
            start: project.track_end("V1"),
            len: info.duration_s,
            src_in: 0.0,
            fx: Default::default(),
            kf: Default::default(),
        };
        project
            .set_media(&info.id, &info.name, &info.path, info.duration_s)
            .map_err(err_str)?;
        project.add_clip(&clip).map_err(err_str)
    })?;
    state.media.lock().unwrap().insert(info.id.clone(), info);
    Ok(json!({ "media": media_value, "project": snap }))
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
    with_undo(&state, |p| p.move_clip(&id, &track, start).map_err(err_str))
}

#[tauri::command]
fn trim_clip(
    id: String,
    start: f64,
    len: f64,
    src_in: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.trim_clip(&id, start, len, src_in).map_err(err_str))
}

#[tauri::command]
fn remove_clip(
    id: String,
    ripple: bool,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| {
        if ripple {
            p.remove_clip_ripple(&id).map_err(err_str)
        } else {
            p.remove_clip(&id).map_err(err_str)
        }
    })
}

#[tauri::command]
fn undo(state: State<AppState>) -> Result<serde_json::Value, String> {
    let entry = state.history.lock().unwrap().undo.pop();
    let Some((before, after)) = entry else {
        return Ok(state.project.lock().unwrap().snapshot());
    };
    let mut project = state.project.lock().unwrap();
    project.restore_clips(&before).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    state.history.lock().unwrap().redo.push((before, after));
    notify_sync(&state);
    Ok(snap)
}

#[tauri::command]
fn redo(state: State<AppState>) -> Result<serde_json::Value, String> {
    let entry = state.history.lock().unwrap().redo.pop();
    let Some((before, after)) = entry else {
        return Ok(state.project.lock().unwrap().snapshot());
    };
    let mut project = state.project.lock().unwrap();
    project.restore_clips(&after).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    state.history.lock().unwrap().undo.push((before, after));
    notify_sync(&state);
    Ok(snap)
}

/// One logical edit that razors many source ranges (silence / filler
/// removal) — a single undo entry.
#[tauri::command]
fn cut_ranges(
    media_id: String,
    ranges: Vec<(f64, f64)>,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    with_undo(&state, |p| {
        p.razor_out_ranges(&media_id, &ranges, &format!("c{nanos:x}"))
            .map(|_| ())
            .map_err(err_str)
    })
}

/// Set one Effect Controls parameter on a clip (undoable).
#[tauri::command]
fn set_effect(
    id: String,
    key: String,
    value: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.set_effect(&id, &key, value).map_err(err_str))
}

/// Add/update a keyframe for a param at clip-relative time `t` (undoable).
#[tauri::command]
fn set_keyframe(
    id: String,
    param: String,
    t: f64,
    value: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.set_keyframe(&id, &param, t, value).map_err(err_str))
}

/// Remove all keyframes for a param (revert to its constant value).
#[tauri::command]
fn clear_keyframes(
    id: String,
    param: String,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.clear_keyframes(&id, &param).map_err(err_str))
}

/// Set (or clear, dur=0) a transition INTO a clip from its left neighbor.
/// Both params in one undoable edit.
#[tauri::command]
fn set_transition(
    id: String,
    dur: f64,
    dip: bool,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| {
        p.set_effect(&id, "trans_dur", dur).map_err(err_str)?;
        p.set_effect(&id, "trans_dip", if dip { 1.0 } else { 0.0 })
            .map_err(err_str)
    })
}

#[tauri::command]
fn save_project(path: String, state: State<AppState>) -> Result<(), String> {
    let bytes = state.project.lock().unwrap().save();
    std::fs::write(&path, bytes).map_err(err_str)
}

/// Load a .cutlass file and rebuild the media pool from the paths stored
/// in the document (scrub proxies come from cache when available).
#[tauri::command]
fn open_project(path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(&path).map_err(err_str)?;
    let mut project = Project::load(&bytes).map_err(err_str)?;
    let entries = project.media_entries();

    let mut media_out = Vec::new();
    let mut media_map = state.media.lock().unwrap();
    media_map.clear();
    for (id, name, src_path, _dur) in entries {
        match import_any(Path::new(&src_path)) {
            Ok(info) => {
                // same path on the same machine hashes to the same id
                if info.id == id {
                    media_out.push(media_json(&info)?);
                    media_map.insert(info.id.clone(), info);
                } else {
                    eprintln!("media id changed for {src_path} (moved file?)");
                }
            }
            Err(e) => eprintln!("media offline: {name} ({src_path}): {e:#}"),
        }
    }
    drop(media_map);

    let snap = project.snapshot();
    *state.project.lock().unwrap() = project;
    *state.history.lock().unwrap() = History::default(); // fresh doc, fresh history
    notify_sync(&state);
    Ok(json!({ "project": snap, "media": media_out }))
}

/// Build thumbs/proxy for a media id that exists in the doc but not in
/// this instance's pool yet (opened project or collab peer).
#[tauri::command]
fn hydrate_media(media_id: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let entry = state
        .project
        .lock()
        .unwrap()
        .media_entries()
        .into_iter()
        .find(|(id, ..)| *id == media_id)
        .ok_or_else(|| format!("media {media_id} not in project"))?;
    let info = import_any(Path::new(&entry.2)).map_err(err_str)?;
    let out = media_json(&info)?;
    state.media.lock().unwrap().insert(media_id, info);
    Ok(out)
}

/// Find the whisper model: CUTLASS_WHISPER_MODEL env var, the installed
/// location (whisper/ beside the exe, where the bundler puts it), or —
/// in dev — walking up from the current dir to vendor/whisper/.
fn whisper_model_path() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("CUTLASS_WHISPER_MODEL") {
        return Ok(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let installed = dir.join("whisper").join("ggml-base.en.bin");
            if installed.exists() {
                return Ok(installed);
            }
        }
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
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    with_undo(&state, |p| {
        p.razor_out(&id, src_from, src_to, &format!("c{nanos:x}"))
            .map_err(err_str)
    })
}

/// Start audio for the V1 track from `from_t`. Returns false (not an
/// error) when audio can't start — the UI then runs its silent local
/// clock, so playback still works on machines with no output device.
#[tauri::command]
fn play(from_t: f64, muted: Option<Vec<String>>, state: State<AppState>) -> bool {
    if let Some(h) = state.playback.lock().unwrap().take() {
        h.stop();
    }
    let muted = muted.unwrap_or_default();
    let tracks: Vec<Vec<cutlass_engine::player::AudioClip>> = {
        let project = state.project.lock().unwrap();
        let media = state.media.lock().unwrap();
        let snap = project.snapshot();
        ["V1", "V2"]
            .iter()
            .filter(|t| !muted.iter().any(|m| m == *t))
            .map(|track| {
                snap["clips"]
                    .as_array()
                    .map(|cs| {
                        cs.iter()
                            .filter(|c| c["track"] == *track)
                            .filter_map(|c| {
                                let m = media.get(c["media"].as_str()?)?;
                                Some(cutlass_engine::player::AudioClip {
                                    path: m.path.clone(),
                                    start: c["start"].as_f64()?,
                                    len: c["len"].as_f64()?,
                                    src_in: c["src_in"].as_f64()?,
                                    volume: c["fx"]["volume"].as_f64().unwrap_or(1.0),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    match cutlass_engine::player::start(tracks, from_t) {
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

/// Render the V1 track to an MP4. Long-running; emits `export-progress`
/// (0..=1). Returns the encoder used (h264_qsv or libx264).
#[tauri::command]
fn export_project(
    path: String,
    width: Option<u32>,
    height: Option<u32>,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Result<String, String> {
    use tauri::Emitter;
    let (clips, overlays) = {
        let mut project = state.project.lock().unwrap();
        let paths: HashMap<String, String> = project
            .media_entries()
            .into_iter()
            .map(|(id, _, p, _)| (id, p))
            .collect();
        let all = project.clips_state();
        use cutlass_core::export::{ClipFx, ExportClip};
        let clips: Vec<ExportClip> = all
            .iter()
            .filter(|c| c.track == "V1")
            .filter_map(|c| {
                Some(ExportClip {
                    start: c.start,
                    len: c.len,
                    src_in: c.src_in,
                    path: paths.get(&c.media)?.clone(),
                    fx: ClipFx::from_map(&c.fx),
                    trans_dur: c.fx.get("trans_dur").copied().unwrap_or(0.0),
                    trans_dip: c.fx.get("trans_dip").copied().unwrap_or(0.0) > 0.5,
                    kf: c
                        .kf
                        .keys()
                        .map(|p| (p.clone(), cutlass_core::project::kf_points(&c.kf, p)))
                        .collect(),
                })
            })
            .collect();
        let overlays: Vec<cutlass_core::export::Overlay> = all
            .iter()
            .filter(|c| c.track == "V2")
            .filter_map(|c| {
                Some(cutlass_core::export::Overlay {
                    path: paths.get(&c.media)?.clone(),
                    src_in: c.src_in,
                    len: c.len,
                    start: c.start,
                    fx: ClipFx::from_map(&c.fx),
                })
            })
            .collect();
        (clips, overlays)
    };
    if clips.is_empty() {
        return Err("nothing on V1 to export".into());
    }
    let segments = cutlass_core::export::build_segments(clips);
    let settings = cutlass_core::export::ExportSettings {
        width: width.unwrap_or(1920),
        height: height.unwrap_or(1080),
        fps: 30,
    };
    cutlass_core::export::export(&segments, &overlays, Path::new(&path), &settings, &mut |p| {
        let _ = app.emit("export-progress", p);
    })
    .map_err(err_str)
}

/// Join (or start) a collab room. The task speaks the Automerge sync
/// protocol with the relay; remote changes land in the shared project
/// and the UI hears about them via the `project-changed` event.
/// Which collab room (if any) this instance is in — lets the UI catch up
/// after a CUTLASS_ROOM auto-join.
#[tauri::command]
fn current_room(state: State<AppState>) -> Option<String> {
    state.room.lock().unwrap().clone()
}

/// Forward an ephemeral presence payload to the room (no-op untethered).
#[tauri::command]
fn send_presence(payload: serde_json::Value, state: State<AppState>) {
    if let Some(tx) = state.sync_tx.lock().unwrap().as_ref() {
        let _ = tx.send(SyncCmd::Presence(payload.to_string()));
    }
}

#[tauri::command]
fn join_session(room: String, app: tauri::AppHandle, state: State<AppState>) -> Result<(), String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SyncCmd>();
    *state.sync_tx.lock().unwrap() = Some(tx);
    *state.room.lock().unwrap() = Some(room.clone());
    let url = std::env::var("CUTLASS_SYNC_URL").unwrap_or_else(|_| "ws://127.0.0.1:9720".into());
    tauri::async_runtime::spawn(sync_task(app, format!("{url}/{room}"), rx));
    Ok(())
}

async fn sync_task(
    app: tauri::AppHandle,
    url: String,
    mut local_edits: tokio::sync::mpsc::UnboundedReceiver<SyncCmd>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tauri::{Emitter, Manager};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let ws = match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            eprintln!("collab connect failed ({url}): {e}");
            let _ = app.emit("collab-error", format!("connect failed: {e}"));
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();
    let mut sync = automerge::sync::State::new();
    let state = app.state::<AppState>();
    let mut out: Vec<Vec<u8>> = Vec::new();

    // offer our current doc
    {
        let mut p = state.project.lock().unwrap();
        while let Some(m) = p.generate_sync_message(&mut sync) {
            out.push(m);
        }
    }
    for m in out.drain(..) {
        let _ = sink.send(WsMessage::Binary(m.into())).await;
    }

    loop {
        tokio::select! {
            msg = stream.next() => {
                match msg {
                    Some(Ok(WsMessage::Binary(bytes))) => {
                        let snap = {
                            let mut p = state.project.lock().unwrap();
                            if p.receive_sync_message(&mut sync, &bytes).is_err() {
                                continue;
                            }
                            while let Some(m) = p.generate_sync_message(&mut sync) {
                                out.push(m);
                            }
                            p.snapshot()
                        };
                        for m in out.drain(..) {
                            let _ = sink.send(WsMessage::Binary(m.into())).await;
                        }
                        let _ = app.emit("project-changed", snap);
                    }
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                            let _ = app.emit("presence", v);
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
            cmd = local_edits.recv() => {
                match cmd {
                    None => break,
                    Some(SyncCmd::Presence(json)) => {
                        let _ = sink.send(WsMessage::Text(json.into())).await;
                    }
                    Some(SyncCmd::Ping) => {
                        {
                            let mut p = state.project.lock().unwrap();
                            while let Some(m) = p.generate_sync_message(&mut sync) {
                                out.push(m);
                            }
                        }
                        for m in out.drain(..) {
                            let _ = sink.send(WsMessage::Binary(m.into())).await;
                        }
                    }
                }
            }
        }
    }
    eprintln!("collab session ended ({url})");
    let _ = app.emit("collab-error", "session ended");
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            // CUTLASS_ROOM=<name> auto-joins a collab room at startup
            if let Ok(room) = std::env::var("CUTLASS_ROOM") {
                use tauri::Manager;
                let handle = app.handle().clone();
                let state = app.state::<AppState>();
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SyncCmd>();
                *state.sync_tx.lock().unwrap() = Some(tx);
                *state.room.lock().unwrap() = Some(room.clone());
                let url = std::env::var("CUTLASS_SYNC_URL")
                    .unwrap_or_else(|_| "ws://127.0.0.1:9720".into());
                tauri::async_runtime::spawn(sync_task(handle, format!("{url}/{room}"), rx));
            }
            Ok(())
        })
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
            razor_out,
            save_project,
            open_project,
            hydrate_media,
            join_session,
            send_presence,
            current_room,
            export_project,
            undo,
            redo,
            cut_ranges,
            set_effect,
            set_transition,
            set_keyframe,
            clear_keyframes
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}
