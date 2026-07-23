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
    playback: Mutex<Option<cutlass_engine::player::PlaybackHandle>>,
    /// stop flag for the real-time video playback thread
    video_stop: Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    /// cancel flag for the in-flight export render
    export_cancel: Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    /// pings the collab task after a local edit, when a session is live
    sync_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<SyncCmd>>>,
    room: Mutex<Option<String>>,
    /// a .cutlass path the app was launched with (double-clicked file),
    /// consumed once by the frontend on mount via `take_startup_file`
    startup_file: Mutex<Option<String>>,
}

/// Pull the first `.cutlass` path out of a launch argv (skips the exe).
fn cutlass_arg(argv: &[String]) -> Option<String> {
    argv.iter()
        .skip(1)
        .find(|a| a.to_lowercase().ends_with(".cutlass"))
        .cloned()
}

/// A video clip as the playback thread sees it (visible tracks only).
#[derive(Clone)]
struct PlayClip {
    path: String,
    start: f64,
    len: f64,
    src_in: f64,
    speed: f64,
    track_pri: u8, // higher wins the monitor (Vn over ... over V1)
}

/// Track names are "V1".."Vn" (video, composited bottom→top) or
/// "A1".."An" (audio-only beds). These parse the kind and z-order.
fn track_is_audio(t: &str) -> bool {
    matches!(t.chars().next(), Some('A') | Some('a'))
}
/// Video z-order: "V2" → 2 (higher composites on top). Audio tracks → 0.
fn track_video_pri(t: &str) -> u8 {
    if track_is_audio(t) {
        return 0;
    }
    t.trim_start_matches(|c: char| c.is_alphabetic())
        .parse::<u8>()
        .unwrap_or(0)
}
/// 1-based index in a track name ("V3" → 3, "A2" → 2), either kind.
fn track_num(t: &str) -> u32 {
    t.trim_start_matches(|c: char| c.is_alphabetic())
        .parse::<u32>()
        .unwrap_or(0)
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
    // Aim for ~480 proxy frames (dense on short clips, capped on long
    // ones so an hour is ~8s/frame instead of 15s), at most 10 fps.
    let scrub_fps = (480.0 / duration_s.max(0.1)).min(10.0);
    let interval = 1.0 / scrub_fps;
    let width = cutlass_core::media::SCRUB_WIDTH;
    let dir = cutlass_core::media::cache_dir(path)?;
    let mut thumb_paths = cutlass_core::media::read_frames(&dir)?;

    let encode = |f: &cutlass_engine::RgbaFrame, i: u32| -> anyhow::Result<()> {
        let rgb: Vec<u8> = f.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        let mut out = std::fs::File::create(dir.join(format!("f{i:05}.jpg")))?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode(&rgb, f.width, f.height, image::ExtendedColorType::Rgb8)?;
        Ok(())
    };

    if thumb_paths.is_empty() {
        if interval > 1.5 {
            // Long clip: SEEK to each thumbnail time (keyframe-fast). One
            // sequential decode of an hour would be minutes; ~480 seeks
            // is seconds.
            let n = (duration_s * scrub_fps).ceil() as u32;
            for i in 1..=n {
                let t = (i - 1) as f64 * interval;
                match eng.keyframe_at(t, width) {
                    Ok(f) => encode(&f, i)?,
                    Err(_) => break,
                }
            }
        } else {
            // Short clip: one sequential pass is faster than many seeks.
            let mut i = 0u32;
            eng.sample_frames(interval, width, |f| {
                i += 1;
                encode(&f, i)
            })?;
        }
        thumb_paths = cutlass_core::media::read_frames(&dir)?;
        anyhow::ensure!(!thumb_paths.is_empty(), "no frames sampled");
    }

    // Waveform is a full audio decode; skip it for very long clips to
    // keep import responsive (they can still be edited without it).
    let waveform = if duration_s <= 1800.0 {
        cutlass_engine::audio::waveform_peaks(&path_str, 1200)
    } else {
        Vec::new()
    };

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
        waveform,
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

/// Import a video: probe, build scrub proxy, register it in the media
/// pool. Does NOT drop a clip on the timeline — the media lands in the bin
/// and the user drags it onto a track when they want it (see
/// `add_clip_from_media`). ASYNC + spawn_blocking so the (potentially
/// seconds-long) proxy build never blocks the UI thread.
#[tauri::command]
async fn import_media(
    path: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let info = tauri::async_runtime::spawn_blocking(move || import_any(Path::new(&path)))
        .await
        .map_err(err_str)?
        .map_err(err_str)?;
    let media_value = media_json(&info)?;

    // register in the CRDT media pool (persists + syncs) without touching
    // the timeline — clips_state is unchanged so this adds no undo entry
    let snap = with_undo(&state, |project| {
        project
            .set_media(&info.id, &info.name, &info.path, info.duration_s)
            .map_err(err_str)
    })?;
    state.media.lock().unwrap().insert(info.id.clone(), info);
    Ok(json!({ "media": media_value, "project": snap }))
}

/// Place a clip on the timeline from already-imported pool media, at the
/// given track and start (undoable). This is what a drag-and-drop from the
/// media bin calls. Returns the new snapshot and the created clip id.
#[tauri::command]
fn add_clip_from_media(
    media_id: String,
    track: String,
    start: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let (name, dur) = {
        let media = state.media.lock().unwrap();
        let info = media.get(&media_id).ok_or("unknown media")?;
        (info.name.clone(), info.duration_s)
    };
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let id = format!("c{nanos:x}");
    let clip = Clip {
        id: id.clone(),
        name,
        media: media_id,
        track,
        start: start.max(0.0),
        len: dur,
        src_in: 0.0,
        text: String::new(),
        lut: String::new(),
        fx: Default::default(),
        kf: Default::default(),
    };
    let snap = with_undo(&state, |project| project.add_clip(&clip).map_err(err_str))?;
    Ok(json!({ "project": snap, "clipId": id }))
}

/// Remove a track: delete its clips, then shift the higher same-kind
/// tracks down one so numbering stays contiguous (V4→V3, …). Undoable.
#[tauri::command]
fn remove_track(track: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let audio = track_is_audio(&track);
    let removed = track_num(&track);
    let kind = if audio { "A" } else { "V" };
    with_undo(&state, |p| {
        for id in p
            .clips_state()
            .iter()
            .filter(|c| c.track == track)
            .map(|c| c.id.clone())
            .collect::<Vec<_>>()
        {
            p.remove_clip(&id).map_err(err_str)?;
        }
        // renumber clips on higher tracks of the same kind down by one
        let shifts: Vec<(String, String, f64)> = p
            .clips_state()
            .iter()
            .filter(|c| track_is_audio(&c.track) == audio && track_num(&c.track) > removed)
            .map(|c| (c.id.clone(), format!("{kind}{}", track_num(&c.track) - 1), c.start))
            .collect();
        for (id, new_track, start) in shifts {
            p.move_clip(&id, &new_track, start).map_err(err_str)?;
        }
        Ok(())
    })
}

#[tauri::command]
fn get_project(state: State<AppState>) -> serde_json::Value {
    state.project.lock().unwrap().snapshot()
}

/// Read a small text file (used to load a .cube LUT for the GPU preview).
#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(err_str)
}

/// Open a URL (or mailto:) in the OS default handler — used by feedback.
#[tauri::command]
fn open_url(url: String) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}

/// Reveal a file in the OS file browser (selects it in its folder).
#[tauri::command]
fn reveal_file(path: String) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
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

/// Split a clip at timeline position `at` (the blade tool).
#[tauri::command]
fn split_clip(id: String, at: f64, state: State<AppState>) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    with_undo(&state, |p| p.split_clip(&id, at, &format!("s{nanos:x}")).map_err(err_str))
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

/// Apply several fx params at once (one undo step) — used by the Effects
/// tab to drop a whole "Look" or effect preset onto a clip.
#[tauri::command]
fn set_effects(
    id: String,
    params: std::collections::HashMap<String, f64>,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| {
        for (key, value) in &params {
            p.set_effect(&id, key, *value).map_err(err_str)?;
        }
        Ok(())
    })
}

/// Create a title (text) clip on V2 at `start`, default lower-third style.
#[tauri::command]
fn add_title(start: f64, state: State<AppState>) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    with_undo(&state, |p| {
        let mut fx = std::collections::BTreeMap::new();
        fx.insert("pos_y".to_string(), 0.3);
        fx.insert("font_size".to_string(), 56.0);
        fx.insert("title_bg".to_string(), 0.5);
        p.add_clip(&Clip {
            id: format!("t{nanos:x}"),
            name: "Title".into(),
            media: String::new(),
            track: "V2".into(),
            start,
            len: 4.0,
            src_in: 0.0,
            text: "Title".into(),
            lut: String::new(),
            fx,
            kf: Default::default(),
        })
        .map_err(err_str)
    })
}

#[tauri::command]
fn set_title_text(
    id: String,
    text: String,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.set_title_text(&id, &text).map_err(err_str))
}

/// Set (or clear with "") the .cube LUT applied to a clip (undoable).
#[tauri::command]
fn set_lut(id: String, path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| p.set_lut(&id, &path).map_err(err_str))
}

#[derive(serde::Deserialize)]
struct CaptionSpec {
    text: String,
    start: f64,
    len: f64,
}

/// Drop a batch of caption clips (styled text clips on V2) from the
/// transcript — one undo step. Each caption is a lower-third title with a
/// background band, timed to its words.
#[tauri::command]
fn add_captions(
    captions: Vec<CaptionSpec>,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let base = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    with_undo(&state, |p| {
        for (i, c) in captions.iter().enumerate() {
            let mut fx = std::collections::BTreeMap::new();
            fx.insert("pos_y".to_string(), 0.34); // lower third
            fx.insert("font_size".to_string(), 46.0);
            fx.insert("title_bg".to_string(), 0.55);
            p.add_clip(&Clip {
                id: format!("cap{:x}", base + i as u128),
                name: "Caption".into(),
                media: String::new(),
                track: "V2".into(),
                start: c.start,
                len: c.len.max(0.3),
                src_in: 0.0,
                text: c.text.clone(),
                lut: String::new(),
                fx,
                kf: Default::default(),
            })
            .map_err(err_str)?;
        }
        Ok(())
    })
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
fn save_project(path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock().unwrap();
    // name the project after the file so the title stops reading "Untitled"
    if let Some(stem) = Path::new(&path).file_stem().and_then(|s| s.to_str()) {
        project.set_name(stem);
    }
    let bytes = project.save();
    std::fs::write(&path, bytes).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    notify_sync(&state);
    Ok(snap)
}

/// The .cutlass path the app was launched with (double-clicked file), if
/// any. Returns it once then clears it — the frontend loads it on mount.
#[tauri::command]
fn take_startup_file(state: State<AppState>) -> Option<String> {
    state.startup_file.lock().unwrap().take()
}

/// App preferences (e.g. auto-save) live in a small JSON file on disk so
/// they survive restarts — WebView localStorage does not reliably persist.
fn settings_file(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_config_dir().map_err(err_str)?;
    Ok(dir.join("settings.json"))
}

#[tauri::command]
fn load_prefs(app: tauri::AppHandle) -> serde_json::Value {
    settings_file(&app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

#[tauri::command]
fn save_pref(app: tauri::AppHandle, key: String, value: serde_json::Value) -> Result<(), String> {
    let path = settings_file(&app)?;
    let mut obj = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(map) = obj.as_object_mut() {
        map.insert(key, value);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err_str)?;
    }
    let text = serde_json::to_string_pretty(&obj).map_err(err_str)?;
    std::fs::write(&path, text).map_err(err_str)
}

/// Load a .cutlass file and rebuild the media pool from the paths stored
/// in the document (scrub proxies come from cache when available).
#[tauri::command]
async fn open_project(
    path: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(&path).map_err(err_str)?;
    let mut project = Project::load(&bytes).map_err(err_str)?;
    let entries = project.media_entries();

    // rebuilding each media (proxy/waveform) is heavy → off the UI thread
    let media_pairs = tauri::async_runtime::spawn_blocking(
        move || -> Vec<(MediaInfo, serde_json::Value)> {
            let mut pairs = Vec::new();
            for (id, name, src_path, _dur) in entries {
                match import_any(Path::new(&src_path)) {
                    // same path on the same machine hashes to the same id
                    Ok(info) if info.id == id => {
                        if let Ok(v) = media_json(&info) {
                            pairs.push((info, v));
                        }
                    }
                    Ok(_) => eprintln!("media id changed for {src_path} (moved file?)"),
                    Err(e) => eprintln!("media offline: {name} ({src_path}): {e:#}"),
                }
            }
            pairs
        },
    )
    .await
    .map_err(err_str)?;

    let mut media_out = Vec::new();
    let mut media_map = state.media.lock().unwrap();
    media_map.clear();
    for (info, v) in media_pairs {
        media_out.push(v);
        media_map.insert(info.id.clone(), info);
    }
    drop(media_map);

    // stored transcripts (media_id → [{text,start,end}]) so the words come
    // back on reopen instead of forcing a re-transcribe
    let transcripts: serde_json::Map<String, serde_json::Value> = project
        .transcripts()
        .into_iter()
        .filter_map(|(id, j)| serde_json::from_str::<serde_json::Value>(&j).ok().map(|v| (id, v)))
        .collect();

    let snap = project.snapshot();
    *state.project.lock().unwrap() = project;
    *state.history.lock().unwrap() = History::default(); // fresh doc, fresh history
    notify_sync(&state);
    Ok(json!({ "project": snap, "media": media_out, "transcripts": transcripts }))
}

/// Build thumbs/proxy for a media id that exists in the doc but not in
/// this instance's pool yet (opened project or collab peer).
#[tauri::command]
async fn hydrate_media(
    media_id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let entry = state
        .project
        .lock()
        .unwrap()
        .media_entries()
        .into_iter()
        .find(|(id, ..)| *id == media_id)
        .ok_or_else(|| format!("media {media_id} not in project"))?;
    let src = entry.2;
    let (info, out) = tauri::async_runtime::spawn_blocking(
        move || -> Result<(MediaInfo, serde_json::Value), String> {
            let info = import_any(Path::new(&src)).map_err(err_str)?;
            let out = media_json(&info)?;
            Ok((info, out))
        },
    )
    .await
    .map_err(err_str)??;
    state.media.lock().unwrap().insert(media_id, info);
    Ok(out)
}

/// Find the whisper model: CUTLASS_WHISPER_MODEL env var, then a few
/// layouts beside the exe (the bundler drops the model there), then —
/// in dev — walking up from the current dir to vendor/whisper/.
fn whisper_model_path() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("CUTLASS_WHISPER_MODEL") {
        return Ok(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Try, in order: flat beside the exe (current bundling), the
            // intended whisper/ subfolder, and the legacy layout where a
            // trailing-slash resource map produced a *file* named "whisper".
            for cand in [
                dir.join("ggml-base.en.bin"),
                dir.join("whisper").join("ggml-base.en.bin"),
                dir.join("whisper"),
            ] {
                if cand.is_file() {
                    return Ok(cand);
                }
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
            return Err("whisper model not found (looked beside the app and in vendor/whisper/)".into());
        }
    }
}

/// On-device transcription with word timestamps. Slow-ish (≈ 1/5 of the
/// clip duration on CPU); runs on a worker thread.
#[tauri::command]
async fn transcribe_media(
    media_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<cutlass_engine::transcribe::Word>, String> {
    let path = state
        .media
        .lock()
        .unwrap()
        .get(&media_id)
        .map(|m| m.path.clone())
        .ok_or_else(|| format!("unknown media {media_id}"))?;
    let model = whisper_model_path()?;
    // whisper runs seconds+ on CPU — off the UI thread
    let words = tauri::async_runtime::spawn_blocking(move || {
        cutlass_engine::transcribe::transcribe(&path, &model.to_string_lossy()).map_err(err_str)
    })
    .await
    .map_err(err_str)??;
    // persist into the doc so the transcript saves + syncs with the project
    if let Ok(json) = serde_json::to_string(&words) {
        let _ = state.project.lock().unwrap().set_transcript(&media_id, &json);
        notify_sync(&state);
    }
    Ok(words)
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
fn play(
    from_t: f64,
    muted: Option<Vec<String>>,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> bool {
    use std::sync::atomic::Ordering;
    if let Some(h) = state.playback.lock().unwrap().take() {
        h.stop();
    }
    if let Some(v) = state.video_stop.lock().unwrap().take() {
        v.store(true, Ordering::Relaxed);
    }
    let muted = muted.unwrap_or_default();

    // ── real-time video playback thread ────────────────────────────────
    let video_clips: Vec<PlayClip> = {
        let project = state.project.lock().unwrap();
        let media = state.media.lock().unwrap();
        let snap = project.snapshot();
        snap["clips"]
            .as_array()
            .map(|cs| {
                cs.iter()
                    .filter(|c| c["text"].as_str().unwrap_or("").is_empty()) // not titles
                    .filter(|c| !track_is_audio(c["track"].as_str().unwrap_or(""))) // video tracks only
                    .filter_map(|c| {
                        let m = media.get(c["media"].as_str()?)?;
                        let track = c["track"].as_str()?;
                        Some(PlayClip {
                            path: m.path.clone(),
                            start: c["start"].as_f64()?,
                            len: c["len"].as_f64()?,
                            src_in: c["src_in"].as_f64()?,
                            speed: c["fx"]["speed"].as_f64().unwrap_or(1.0),
                            track_pri: track_video_pri(track),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    if !video_clips.is_empty() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *state.video_stop.lock().unwrap() = Some(stop.clone());
        start_video_thread(app, video_clips, from_t, stop);
    }
    let tracks: Vec<Vec<cutlass_engine::player::AudioClip>> = {
        let project = state.project.lock().unwrap();
        let media = state.media.lock().unwrap();
        let snap = project.snapshot();
        // every track that carries clips contributes audio — video tracks
        // (their embedded audio) and audio-only beds alike
        let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if let Some(cs) = snap["clips"].as_array() {
            for c in cs {
                if let Some(t) = c["track"].as_str() {
                    names.insert(t.to_string());
                }
            }
        }
        names
            .iter()
            .filter(|t| !muted.iter().any(|m| m == *t))
            .map(|track| {
                snap["clips"]
                    .as_array()
                    .map(|cs| {
                        cs.iter()
                            .filter(|c| c["track"].as_str() == Some(track.as_str()))
                            .filter(|c| c["text"].as_str().unwrap_or("").is_empty()) // titles are silent
                            .filter_map(|c| {
                                let m = media.get(c["media"].as_str()?)?;
                                Some(cutlass_engine::player::AudioClip {
                                    path: m.path.clone(),
                                    start: c["start"].as_f64()?,
                                    len: c["len"].as_f64()?,
                                    src_in: c["src_in"].as_f64()?,
                                    volume: c["fx"]["volume"].as_f64().unwrap_or(1.0),
                                    speed: c["fx"]["speed"].as_f64().unwrap_or(1.0),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    // If nothing has playable audio at all (e.g. the only clip under way
    // is retimed, so it's filtered out above), don't start the audio
    // engine — an empty stream reports "ended" immediately and would stop
    // playback. Returning false makes the UI run its silent local clock so
    // the video thread keeps streaming to the end of the content.
    if tracks.iter().all(|t| t.is_empty()) {
        return false;
    }
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

/// Stop audio + video playback; returns where audio stopped.
#[tauri::command]
fn pause(state: State<AppState>) -> Option<f64> {
    if let Some(v) = state.video_stop.lock().unwrap().take() {
        v.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
async fn exact_frame(path: String, t: f64) -> Result<String, String> {
    // frame-accurate decode off the UI thread — a settle never freezes it
    tauri::async_runtime::spawn_blocking(move || {
        let mut engine = cutlass_engine::MediaEngine::open(&path).map_err(err_str)?;
        let frame = engine.frame_at(t, 1920).map_err(err_str)?;
        frame_to_data_url(&frame, 88).map_err(err_str)
    })
    .await
    .map_err(err_str)?
}

/// RGBA frame → JPEG data URL (drops alpha; the encoder rejects Rgba8).
fn frame_to_data_url(frame: &cutlass_engine::RgbaFrame, quality: u8) -> anyhow::Result<String> {
    let rgb: Vec<u8> = frame
        .data
        .chunks_exact(4)
        .flat_map(|px| [px[0], px[1], px[2]])
        .collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
        .encode(&rgb, frame.width, frame.height, image::ExtendedColorType::Rgb8)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    ))
}

/// Real-time video playback: a dedicated thread decodes the visible clip
/// under the moving playhead and streams JPEG frames to the UI at ~30fps.
/// Paced by wall clock from `from_t` (matching the audio, also wall-paced).
fn start_video_thread(
    app: tauri::AppHandle,
    clips: Vec<PlayClip>,
    from_t: f64,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use tauri::Emitter;
    std::thread::Builder::new()
        .name("cutlass-video-playback".into())
        .spawn(move || {
            let mut engines: HashMap<String, cutlass_engine::MediaEngine> = HashMap::new();
            let frame_dur = Duration::from_millis(33); // ~30 fps
            let start = Instant::now();
            let mut next = start + frame_dur;
            while !stop.load(Ordering::Relaxed) {
                let t = from_t + start.elapsed().as_secs_f64();
                // topmost visible clip under the playhead (V2 over V1)
                let active = clips
                    .iter()
                    .filter(|c| t >= c.start && t < c.start + c.len)
                    .max_by_key(|c| c.track_pri);
                if let Some(c) = active {
                    let src_t = c.src_in + (t - c.start) * c.speed;
                    if !engines.contains_key(&c.path) {
                        if let Ok(e) = cutlass_engine::MediaEngine::open(&c.path) {
                            engines.insert(c.path.clone(), e);
                        }
                    }
                    if let Some(eng) = engines.get_mut(&c.path) {
                        if let Ok(frame) = eng.stream_frame_at(src_t, 854) {
                            if let Ok(url) = frame_to_data_url(&frame, 72) {
                                let _ = app.emit("playback-frame", json!({ "t": t, "src": url }));
                            }
                        }
                    }
                }
                // pace to the frame grid, catching up if we fell behind
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                next += frame_dur;
                if next < Instant::now() {
                    next = Instant::now() + frame_dur;
                }
            }
        })
        .ok();
}

/// Render the V1 track to an MP4. Long-running; emits `export-progress`
/// (0..=1). Returns the encoder used (h264_qsv or libx264).
#[tauri::command]
async fn export_project(
    path: String,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    format: Option<String>,
    quality: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use tauri::Emitter;
    let (clips, overlays, titles) = {
        let mut project = state.project.lock().unwrap();
        let paths: HashMap<String, String> = project
            .media_entries()
            .into_iter()
            .map(|(id, _, p, _)| (id, p))
            .collect();
        let all = project.clips_state();
        use cutlass_core::export::{ClipFx, ExportClip, Title};
        let titles: Vec<Title> = all
            .iter()
            .filter(|c| !c.text.is_empty())
            .map(|c| Title {
                text: c.text.clone(),
                start: c.start,
                len: c.len,
                pos_x: c.fx.get("pos_x").copied().unwrap_or(0.0),
                pos_y: c.fx.get("pos_y").copied().unwrap_or(0.25),
                font_size: c.fx.get("font_size").copied().unwrap_or(56.0),
                bg: c.fx.get("title_bg").copied().unwrap_or(0.0),
            })
            .collect();
        // base program = the lowest-index video track carrying real clips;
        // higher video tracks composite over it, audio tracks mix in as beds
        let base_pri = all
            .iter()
            .filter(|c| c.text.is_empty() && !track_is_audio(&c.track))
            .map(|c| track_video_pri(&c.track))
            .min()
            .unwrap_or(1);
        let clips: Vec<ExportClip> = all
            .iter()
            .filter(|c| {
                c.text.is_empty()
                    && !track_is_audio(&c.track)
                    && track_video_pri(&c.track) == base_pri
            })
            .filter_map(|c| {
                Some(ExportClip {
                    start: c.start,
                    len: c.len,
                    src_in: c.src_in,
                    path: paths.get(&c.media)?.clone(),
                    fx: ClipFx::from_map(&c.fx),
                    lut: c.lut.clone(),
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
        // higher video tracks → video overlays, ascending so they stack in
        // z-order; A-tracks → audio-only beds
        let mut higher: Vec<&cutlass_core::project::Clip> = all
            .iter()
            .filter(|c| {
                c.text.is_empty()
                    && !track_is_audio(&c.track)
                    && track_video_pri(&c.track) > base_pri
            })
            .collect();
        higher.sort_by_key(|c| track_video_pri(&c.track));
        let mut overlays: Vec<cutlass_core::export::Overlay> = higher
            .iter()
            .filter_map(|c| {
                Some(cutlass_core::export::Overlay {
                    path: paths.get(&c.media)?.clone(),
                    src_in: c.src_in,
                    len: c.len,
                    start: c.start,
                    fx: ClipFx::from_map(&c.fx),
                    audio_only: false,
                })
            })
            .collect();
        for c in all
            .iter()
            .filter(|c| c.text.is_empty() && track_is_audio(&c.track))
        {
            if let Some(p) = paths.get(&c.media) {
                overlays.push(cutlass_core::export::Overlay {
                    path: p.clone(),
                    src_in: c.src_in,
                    len: c.len,
                    start: c.start,
                    fx: ClipFx::from_map(&c.fx),
                    audio_only: true,
                });
            }
        }
        (clips, overlays, titles)
    };
    if clips.is_empty() {
        return Err("nothing to export — add a video clip to the timeline".into());
    }
    let segments = cutlass_core::export::build_segments(clips);
    let settings = cutlass_core::export::ExportSettings {
        width: width.unwrap_or(1920),
        height: height.unwrap_or(1080),
        fps: fps.unwrap_or(30),
        format: cutlass_core::export::ExportFormat::parse(format.as_deref().unwrap_or("mp4_h264")),
        quality: cutlass_core::export::Quality::parse(quality.as_deref().unwrap_or("medium")),
    };
    // cancel handle: cancel_export flips this; the render loop kills ffmpeg
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    *state.export_cancel.lock().unwrap() = Some(cancel.clone());
    // the render runs minutes — off the UI thread; progress still streams
    let result = tauri::async_runtime::spawn_blocking(move || {
        cutlass_core::export::export(
            &segments,
            &overlays,
            &titles,
            Path::new(&path),
            &settings,
            &mut |p| {
                let _ = app.emit("export-progress", p);
            },
            &cancel,
        )
        .map_err(err_str)
    })
    .await
    .map_err(err_str)?;
    *state.export_cancel.lock().unwrap() = None; // done — drop the handle
    result
}

/// Cancel the in-flight export (kills the ffmpeg render).
#[tauri::command]
fn cancel_export(state: State<AppState>) {
    if let Some(c) = state.export_cancel.lock().unwrap().as_ref() {
        c.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
    // A double-clicked .cutlass file arrives as a launch argument; stash it
    // so the frontend can load it once the window is up.
    let state = AppState::default();
    if let Some(f) = cutlass_arg(&std::env::args().collect::<Vec<_>>()) {
        *state.startup_file.lock().unwrap() = Some(f);
    }

    tauri::Builder::default()
        // single-instance MUST be the first plugin: a second launch (e.g.
        // double-clicking another .cutlass) focuses this window and hands
        // it the file instead of opening a whole new Cutlass.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            use tauri::{Emitter, Manager};
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
                let _ = w.unminimize();
            }
            if let Some(f) = cutlass_arg(&argv) {
                let _ = app.emit("open-file", f);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
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
            add_clip_from_media,
            remove_track,
            reveal_file,
            read_text_file,
            open_url,
            cancel_export,
            set_effects,
            set_lut,
            add_captions,
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
            take_startup_file,
            load_prefs,
            save_pref,
            open_project,
            hydrate_media,
            join_session,
            send_presence,
            current_room,
            export_project,
            undo,
            redo,
            cut_ranges,
            split_clip,
            set_effect,
            set_transition,
            set_keyframe,
            clear_keyframes,
            add_title,
            set_title_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}
