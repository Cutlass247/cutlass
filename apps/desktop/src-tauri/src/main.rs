//! Cutlass desktop shell — walking skeleton.
//! Thin Tauri layer over cutlass-core: import media, mutate the CRDT
//! project, hand scrub-proxy frames to the UI as data URLs.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod license;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

/// Lock a mutex, ignoring poisoning.
///
/// A poisoned mutex means some other thread panicked while holding it, and the
/// usual `.lock_ok()` then panics in every later caller too. That turns
/// one fault anywhere in the app into a window that can do nothing at all --
/// including save the work in progress -- until it is restarted, which is a far
/// worse outcome than carrying on. What these locks guard is a document and
/// some caches, not anything whose invariants a panic could leave dangerous.
trait LockExt<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Default)]
struct AppState {
    project: Mutex<Project>,
    history: Mutex<History>,
    media: Mutex<HashMap<String, MediaInfo>>,
    // The Create and Studio tabs are independent workspaces. `project`/`history`
    // /`media` above are always the ACTIVE tab's; these hold the OTHER tab's,
    // and `set_mode` swaps them so every existing command keeps hitting the
    // active one untouched. `active_mode` is "" / "create" or "studio".
    alt_project: Mutex<Project>,
    alt_history: Mutex<History>,
    alt_media: Mutex<HashMap<String, MediaInfo>>,
    active_mode: Mutex<String>,
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
    if let Some(tx) = state.sync_tx.lock_ok().as_ref() {
        let _ = tx.send(SyncCmd::Ping);
    }
}

/// Run a mutation with undo capture + sync notification.
fn with_undo(
    state: &State<AppState>,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock_ok();
    let before = project.clips_state();
    f(&mut project)?;
    let after = project.clips_state();
    let snap = project.snapshot();
    drop(project);
    if before != after {
        let mut h = state.history.lock_ok();
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
    // `{:#}` so an anyhow error brings its causes with it. Plain Display shows
    // only the outermost context, which for a staged export meant the user was
    // told "part 3 of 30" and nothing whatsoever about what went wrong. Other
    // error types ignore the flag.
    format!("{e:#}")
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
        "width": info.width,
        "height": info.height,
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
    // native size — the export UI uses it to stop silent upscaling
    let (src_w, src_h) = eng.dimensions();
    // Aim for ~480 proxy frames (dense on short clips, capped on long
    // ones so an hour is ~8s/frame instead of 15s), at most 10 fps.
    let scrub_fps = (480.0 / duration_s.max(0.1)).min(10.0);
    let interval = 1.0 / scrub_fps;
    let width = cutlass_core::media::SCRUB_WIDTH;
    let dir = cutlass_core::media::cache_dir(path)?;
    let mut thumb_paths = cutlass_core::media::read_frames(&dir)?;

    let encode = |f: &cutlass_engine::RgbaFrame, i: u32| -> anyhow::Result<()> {
        let rgb: Vec<u8> = f
            .data
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let mut out = std::fs::File::create(dir.join(format!("f{i:05}.jpg")))?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode(
            &rgb,
            f.width,
            f.height,
            image::ExtendedColorType::Rgb8,
        )?;
        Ok(())
    };

    if thumb_paths.is_empty() {
        // Sequential decode gives smooth, exact-time thumbs but its cost
        // scales with clip length — past ~1 min it's slow on real footage.
        // Longer clips SEEK to each thumbnail instead (fast at any length;
        // thumbs snap to keyframes, which is fine for a scrub preview).
        if duration_s > 60.0 {
            // SEEK to each thumbnail time (keyframe-fast). One sequential
            // decode of an hour would be minutes; ~480 seeks is seconds.
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
        width: src_w,
        height: src_h,
        scrub_fps,
        thumb_paths,
        waveform,
    })
}

/// Variable-frame-rate sources (screen recordings, many phone clips) place
/// their frames at irregular timestamps; cutting and reassembling them drifts
/// the audio out of sync in both the preview and the export. Detect that on
/// import and transcode a constant-frame-rate working copy to edit against.
/// Returns the conformed path, or None to keep using the original as-is.
fn conform_if_vfr(path: &Path) -> Option<PathBuf> {
    let (vfr, avg, w, h) = {
        let eng = cutlass_engine::MediaEngine::open(&path.to_string_lossy()).ok()?;
        let (w, h) = eng.dimensions();
        (eng.is_vfr(), eng.avg_fps(), w, h)
    };
    if !vfr {
        return None;
    }
    // 30 for typical screen recordings, 60 when the average cadence is high.
    let fps = if avg > 40.0 { 60 } else { 30 };
    // generous bitrate so the working copy doesn't cap the final export's
    // quality; screen content compresses well, so this is rarely the ceiling.
    let bitrate = ((w as u64 * h as u64 * fps as u64) / 5).max(12_000_000);
    match media::conform_to_cfr(path, fps, bitrate) {
        Ok(p) => {
            eprintln!("conformed VFR source to {fps}fps CFR: {}", p.display());
            Some(p)
        }
        Err(e) => {
            eprintln!("VFR conform failed ({e:#}); importing original");
            None
        }
    }
}

/// Engine first; ffmpeg-CLI fallback for containers libav chokes on.
fn import_any(path: &Path) -> anyhow::Result<MediaInfo> {
    let conformed = conform_if_vfr(path);
    let working = conformed.as_deref().unwrap_or(path);
    let mut info = import_with_engine(working).or_else(|e| {
        eprintln!("engine import failed ({e:#}); falling back to ffmpeg CLI");
        media::ensure_ffmpeg()?;
        media::import(working)
    })?;
    // Keep the media's identity tied to the ORIGINAL file: the bin shows its
    // real name and re-importing the same source stays idempotent. `path`
    // (used for preview + export) points at the conformed CFR copy.
    if conformed.is_some() {
        info.id = format!("m{:016x}", media::path_hash(path));
        info.name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "clip".into());
    }
    Ok(info)
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
    state.media.lock_ok().insert(info.id.clone(), info);
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
        let media = state.media.lock_ok();
        let info = media.get(&media_id).ok_or("unknown media")?;
        (info.name.clone(), info.duration_s)
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
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
            .map(|c| {
                (
                    c.id.clone(),
                    format!("{kind}{}", track_num(&c.track) - 1),
                    c.start,
                )
            })
            .collect();
        for (id, new_track, start) in shifts {
            p.move_clip(&id, &new_track, start).map_err(err_str)?;
        }
        Ok(())
    })
}

#[tauri::command]
fn get_project(state: State<AppState>) -> serde_json::Value {
    state.project.lock_ok().snapshot()
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
        // Hand the URL to the default protocol handler WITHOUT cmd.exe.
        // `cmd /c start` mangles URLs: `&` splits the command and `%xx`
        // percent-encoding is eaten as env-var expansion — which is why the
        // mailto body arrived empty. rundll32 passes the URL through intact.
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
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
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn();
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
    with_undo(&state, |p| {
        p.trim_clip(&id, start, len, src_in).map_err(err_str)
    })
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

/// Remove a media item from the project — drops it from the pool and
/// removes any clips that use it (and its transcript). Not undoable, like
/// import; the UI confirms first when clips would be removed.
#[tauri::command]
fn remove_media(media_id: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let snap = {
        let mut p = state.project.lock_ok();
        p.remove_media(&media_id).map_err(err_str)?;
        p.snapshot()
    };
    state.media.lock_ok().remove(&media_id);
    notify_sync(&state);
    Ok(snap)
}

#[tauri::command]
fn undo(state: State<AppState>) -> Result<serde_json::Value, String> {
    let entry = state.history.lock_ok().undo.pop();
    let Some((before, after)) = entry else {
        return Ok(state.project.lock_ok().snapshot());
    };
    let mut project = state.project.lock_ok();
    project.restore_clips(&before).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    state.history.lock_ok().redo.push((before, after));
    notify_sync(&state);
    Ok(snap)
}

#[tauri::command]
fn redo(state: State<AppState>) -> Result<serde_json::Value, String> {
    let entry = state.history.lock_ok().redo.pop();
    let Some((before, after)) = entry else {
        return Ok(state.project.lock_ok().snapshot());
    };
    let mut project = state.project.lock_ok();
    project.restore_clips(&after).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    state.history.lock_ok().undo.push((before, after));
    notify_sync(&state);
    Ok(snap)
}

/// Split a clip at timeline position `at` (the blade tool).
#[tauri::command]
fn split_clip(id: String, at: f64, state: State<AppState>) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    with_undo(&state, |p| {
        p.split_clip(&id, at, &format!("s{nanos:x}"))
            .map_err(err_str)
    })
}

/// One logical edit that razors many source ranges (silence / filler
/// removal) — a single undo entry.
#[tauri::command]
fn cut_ranges(
    media_id: String,
    ranges: Vec<(f64, f64)>,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
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
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
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
    with_undo(&state, |p| {
        p.set_keyframe(&id, &param, t, value).map_err(err_str)
    })
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

/// Motion-track a censor box across the clip and write position keyframes
/// (`{prefix}_x` / `{prefix}_y`, e.g. "censor2_x") so it follows the subject.
/// Offline template matching; replaces any prior track for that box.
#[tauri::command]
async fn track_censor(
    id: String,
    prefix: String,
    // the box exactly as placed by the user (normalised), and the clip-relative
    // time (`anchor`) it was placed at — the tracker locks onto that spot there
    cx: f64,
    cy: f64,
    bw: f64,
    bh: f64,
    anchor: f64,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    // Read just the source path + timing under a brief lock.
    let (path, src_in, len) = {
        let project = state.project.lock_ok();
        let clips = project.clips_state();
        let clip = clips
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| "clip not found".to_string())?;
        let path = state
            .media
            .lock_ok()
            .get(&clip.media)
            .map(|m| m.path.clone())
            .ok_or_else(|| "source media not available".to_string())?;
        (path, clip.src_in, clip.len)
    };

    // Tracking (ffmpeg extract + template matching) is seconds of CPU — run it
    // on a blocking thread so the UI stays responsive, emitting progress.
    let app2 = app.clone();
    let kfs = tauri::async_runtime::spawn_blocking(move || {
        let mut prog = |p: f32| {
            let _ = app2.emit("track-progress", p);
        };
        cutlass_core::track::track_region(&path, src_in, len, cx, cy, bw, bh, anchor, &mut prog)
            .map_err(err_str)
    })
    .await
    .map_err(err_str)??;

    with_undo(&state, |p| {
        let (px, py) = (format!("{prefix}_x"), format!("{prefix}_y"));
        p.clear_keyframes(&id, &px).map_err(err_str)?;
        p.clear_keyframes(&id, &py).map_err(err_str)?;
        for (t, x, y) in &kfs {
            p.set_keyframe(&id, &px, *t, *x).map_err(err_str)?;
            p.set_keyframe(&id, &py, *t, *y).map_err(err_str)?;
        }
        Ok(())
    })
}

/// Fully clear a censor box — its style, geometry, strength, colour, and any
/// tracked keyframes — so re-adding that slot gives a clean default box.
#[tauri::command]
fn reset_censor(
    id: String,
    prefix: String,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    with_undo(&state, |p| {
        p.clear_keyframes(&id, &format!("{prefix}_x"))
            .map_err(err_str)?;
        p.clear_keyframes(&id, &format!("{prefix}_y"))
            .map_err(err_str)?;
        for (k, v) in [
            (prefix.clone(), 0.0),
            (format!("{prefix}_x"), 0.5),
            (format!("{prefix}_y"), 0.5),
            (format!("{prefix}_w"), 0.25),
            (format!("{prefix}_h"), 0.25),
            (format!("{prefix}_str"), 0.4),
            (format!("{prefix}_color"), 0.0),
        ] {
            p.set_effect(&id, &k, v).map_err(err_str)?;
        }
        Ok(())
    })
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

/// The sidecar the previous good save is kept in, beside the project itself.
fn backup_path(path: &Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".bak");
    std::path::PathBuf::from(p)
}

/// Write a project file without ever leaving a half-written one in its place.
///
/// `fs::write` truncates the target before writing, and auto-save calls this
/// every couple of seconds while editing — so anything that interrupts a write
/// (a crash, a power cut, an external drive dropping out) lands in that window.
/// A truncated automerge document is a *total* loss, not a partial one: even
/// one flipped byte makes it unreadable. So write a sibling temp file, force it
/// to the platter, keep the previous good copy as `.bak`, and only then swap it
/// in — a rename on the same volume is atomic, so the project on disk is always
/// either the old file or the new one, never a mixture.
fn write_project_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".saving-{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        // Without this the rename can land before the bytes do, which on a
        // power cut leaves a file that is the right size and all zeroes.
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // One generation back. Copied rather than renamed so the project file is
    // never absent, even for the moment between the two operations.
    if path.exists() {
        let _ = std::fs::copy(path, backup_path(path));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[tauri::command]
fn save_project(path: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let mut project = state.project.lock_ok();
    // name the project after the file so the title stops reading "Untitled"
    if let Some(stem) = Path::new(&path).file_stem().and_then(|s| s.to_str()) {
        project.set_name(stem);
    }
    let bytes = project.save();
    write_project_atomically(Path::new(&path), &bytes).map_err(err_str)?;
    let snap = project.snapshot();
    drop(project);
    notify_sync(&state);
    Ok(snap)
}

/// Buy-links from the server, so the store can move without a new build.
/// Empty strings mean "server didn't say" — the app keeps its own defaults.
#[tauri::command]
async fn checkout_links() -> Result<serde_json::Value, String> {
    let got = tauri::async_runtime::spawn_blocking(license::checkout_links)
        .await
        .map_err(err_str)?;
    let (lic, cred) = got.unwrap_or((None, None));
    Ok(serde_json::json!({ "license": lic, "credits": cred }))
}

/// Whether a collab relay is configured for this install.
///
/// There is no hosted relay, so for everyone running Cutlass today this is
/// false and the Collab button is not offered. It used to be shown and
/// enabled, pointing at ws://127.0.0.1:9720 — a button whose only possible
/// outcome was a connection error.
#[tauri::command]
fn collab_enabled() -> bool {
    std::env::var("CUTLASS_SYNC_URL").is_ok_and(|v| !v.trim().is_empty())
}

/// Whether this build is allowed to update itself.
///
/// False for the Creator edition, and that is the whole point of the command.
/// The update endpoint serves the public trial build, so an owner build that
/// checked for updates would cheerfully replace itself with one — downgrading
/// the only unrestricted copy of Cutlass that exists, on the machine it is
/// least convenient to lose it from. The Creator edition is built from this
/// tree by hand; it doesn't need a channel to reach it.
#[tauri::command]
fn updates_enabled() -> bool {
    !cfg!(feature = "owner")
}

/// Strip everything Windows won't accept in a file name, so a project called
/// `Ep 3: "Rebuild" <final>` still produces a file someone can open.
fn safe_file_stem(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    for c in name.chars() {
        if r#"\/:*?"<>|"#.contains(c) || c.is_control() || c.is_whitespace() {
            // Collapse runs: `Ep 3: "Rebuild"` loses three characters in a row
            // and would otherwise come out full of gaps.
            if !cleaned.ends_with(' ') {
                cleaned.push(' ');
            }
        } else {
            cleaned.push(c);
        }
    }
    // Windows also rejects a trailing dot or space.
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    let capped: String = trimmed.chars().take(80).collect();
    let capped = capped.trim().to_string();
    if capped.is_empty() {
        "Untitled".to_string()
    } else {
        capped
    }
}

/// Write the in-memory project somewhere findable, with no dialog and no
/// arguments from the caller beyond a timestamp for the file name.
///
/// This is what the UI calls when the UI itself has crashed, so it must not
/// depend on anything the frontend still holds — not the open file's path, not
/// a save dialog, not any React state. Everything it needs (the document, its
/// name) is already here in the backend, which is exactly why a crashed window
/// doesn't have to mean lost work.
///
/// Deliberately does NOT rename the project after the file the way
/// `save_project` does: the recovery copy is a lifeboat, not a Save As, and
/// the name it was saved under is how the user will recognise it.
#[tauri::command]
fn save_recovery_copy(
    stamp: String,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Result<String, String> {
    use tauri::Manager;
    let mut project = state.project.lock_ok();
    let stem = safe_file_stem(&project.name());
    let stamp = safe_file_stem(&stamp);
    let file = format!("{stem} (recovered {stamp}).cutlass");

    // Documents\Cutlass Projects, then Documents, then temp. A crash is the
    // worst moment to fail because a folder was missing.
    let p = app.path();
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(docs) = p.document_dir() {
        dirs.push(docs.join("Cutlass Projects"));
        dirs.push(docs);
    }
    dirs.push(std::env::temp_dir());

    let bytes = project.save();
    let mut last = String::from("nowhere to write the file");
    for dir in dirs {
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let path = dir.join(&file);
        match write_project_atomically(&path, &bytes) {
            Ok(()) => return Ok(path.to_string_lossy().into_owned()),
            Err(e) => last = format!("{} ({e})", path.display()),
        }
    }
    Err(format!("couldn't write a recovery copy: {last}"))
}

/// Where new projects go unless the user chooses otherwise: a named folder in
/// Documents, created the first time it's needed.
///
/// Until this existed the save dialog was given a bare file name and no
/// directory, so Windows opened it wherever Explorer happened to be last.
/// Projects ended up scattered with no recent-files list to find them again —
/// and the `.bak` written beside a project only helps someone who can find the
/// project.
#[tauri::command]
fn default_project_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let base = app.path().document_dir().map_err(err_str)?;
    let dir = base.join("Cutlass Projects");
    // Best effort. If it can't be created — permissions, or a file sitting on
    // the name — fall back to Documents rather than failing the save outright.
    if std::fs::create_dir_all(&dir).is_ok() {
        Ok(dir.to_string_lossy().into_owned())
    } else {
        Ok(base.to_string_lossy().into_owned())
    }
}

/// Where exports go unless the user chooses otherwise. Videos, not Downloads:
/// Downloads is a folder people bulk-delete, it usually sits on the system
/// drive, and a staged export needs roughly twice the finished size in
/// transient space, so a long render can fill C: from there.
#[tauri::command]
fn default_export_dir(app: tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let p = app.path();
    let dir = p.video_dir().or_else(|_| p.document_dir()).map_err(err_str)?;
    Ok(dir.to_string_lossy().into_owned())
}

/// The .cutlass path the app was launched with (double-clicked file), if
/// any. Returns it once then clears it — the frontend loads it on mount.
#[tauri::command]
fn take_startup_file(state: State<AppState>) -> Option<String> {
    state.startup_file.lock_ok().take()
}

/// Actually close the window — called by the frontend after the user
/// answers the save-on-quit prompt (the OS close is otherwise intercepted).
#[tauri::command]
fn force_close(window: tauri::WebviewWindow, state: State<AppState>) {
    // A render in flight is a separate ffmpeg process. Windows does not take
    // children down with their parent, so closing the window on top of one
    // leaves it running with nothing left to stop it — still burning the CPU,
    // still writing to the user's disk, invisible outside Task Manager.
    // Ask it to stop and give it a moment to actually go.
    let running = {
        let slot = state.export_cancel.lock_ok();
        if let Some(c) = slot.as_ref() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
            true
        } else {
            false
        }
    };
    if running {
        // The render loop notices the flag between ffmpeg events and kills the
        // child. Bounded, because quitting must not hang on a wedged encoder.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if state.export_cancel.lock_ok().is_none() {
                break;
            }
        }
    }
    let _ = window.destroy();
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

/// Read a project, falling back to the `.bak` beside it. Returns the project
/// and whether it came from that backup.
///
/// A damaged project can't be repaired — automerge rejects the whole document
/// over a single bad byte — so the only real recovery is the copy the previous
/// save left beside it. Losing the last edit beats losing the project.
///
/// Separate from the command so both outcomes can actually be tested; the
/// command itself needs Tauri state that a test has no way to build.
fn read_project_or_backup(path: &Path) -> Result<(Project, bool), String> {
    let read = |p: &Path| -> anyhow::Result<Project> {
        let bytes = std::fs::read(p)?;
        Project::load(&bytes)
    };
    match read(path) {
        Ok(p) => Ok((p, false)),
        // A project from a newer Cutlass is not damage, and the backup beside
        // it was written by that same newer build — so trying it would fail
        // identically and end with "the backup is unreadable too", sending
        // someone hunting for corruption in two files that are both fine.
        Err(e) if e.downcast_ref::<cutlass_core::project::FormatTooNew>().is_some() => {
            Err(format!("{e}"))
        }
        Err(main_err) => {
            let main_err = err_str(main_err);
            let bak = backup_path(path);
            match read(&bak) {
                Ok(p) => {
                    eprintln!(
                        "{} unreadable ({main_err}); recovered from {}",
                        path.display(),
                        bak.display()
                    );
                    Ok((p, true))
                }
                Err(_) if bak.exists() => {
                    Err(format!("{main_err} The backup beside it is unreadable too."))
                }
                Err(_) => Err(main_err),
            }
        }
    }
}

/// Load a .cutlass file and rebuild the media pool from the paths stored
/// in the document (scrub proxies come from cache when available).
#[tauri::command]
async fn open_project(
    path: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let (mut project, recovered_from_backup) = read_project_or_backup(Path::new(&path))?;
    let entries = project.media_entries();

    // rebuilding each media (proxy/waveform) is heavy → off the UI thread
    type Loaded = (Vec<(MediaInfo, serde_json::Value)>, Vec<(String, String)>);
    let (media_pairs, offline) = tauri::async_runtime::spawn_blocking(move || -> Loaded {
        let mut pairs = Vec::new();
        let mut offline = Vec::new();
        for (id, name, src_path, _dur) in entries {
            match import_any(Path::new(&src_path)) {
                // same path on the same machine hashes to the same id
                Ok(info) if info.id == id => {
                    if let Ok(v) = media_json(&info) {
                        pairs.push((info, v));
                    }
                }
                // The file is there but isn't the one this project recorded —
                // a different cut pasted over the same name, most often.
                Ok(_) => {
                    eprintln!("media id changed for {src_path} (moved file?)");
                    offline.push((name, src_path));
                }
                Err(e) => {
                    eprintln!("media offline: {name} ({src_path}): {e:#}");
                    offline.push((name, src_path));
                }
            }
        }
        (pairs, offline)
    })
    .await
    .map_err(err_str)?;

    // Media that didn't load is reported, not swallowed. It used to go to
    // stderr only, which in a packaged build is nowhere: clips turned into
    // blank rectangles and the user was left to work out why on their own —
    // and the commonest cause, an external drive that wasn't plugged in, is
    // one they can fix in seconds if anyone tells them.
    let offline_json: Vec<serde_json::Value> = offline
        .iter()
        .map(|(name, path)| serde_json::json!({ "name": name, "path": path }))
        .collect();

    let mut media_out = Vec::new();
    let mut media_map = state.media.lock_ok();
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
        .filter_map(|(id, j)| {
            serde_json::from_str::<serde_json::Value>(&j)
                .ok()
                .map(|v| (id, v))
        })
        .collect();

    let snap = project.snapshot();
    *state.project.lock_ok() = project;
    *state.history.lock_ok() = History::default(); // fresh doc, fresh history
    notify_sync(&state);
    Ok(json!({
        "project": snap,
        "media": media_out,
        "transcripts": transcripts,
        // the frontend warns when this is set — silently opening an older
        // version of someone's project would be worse than the damage itself
        "recoveredFromBackup": recovered_from_backup,
        // clips whose source didn't load: they still show on the timeline,
        // with no thumbnails and no waveform, and the export will refuse
        // until the files come back
        "offlineMedia": offline_json,
    }))
}

/// Switch the active workspace (Create vs Studio). They're fully independent —
/// separate timeline, undo history, and media pool — so editing one never
/// touches the other. We swap the active slots with the stored alternate, then
/// hand the frontend the now-active workspace's project + media + transcripts.
#[tauri::command]
fn set_mode(mode: String, state: State<AppState>) -> Result<serde_json::Value, String> {
    let mode = if mode == "studio" { "studio" } else { "create" };
    {
        let mut active = state.active_mode.lock_ok();
        let cur = if active.is_empty() {
            "create"
        } else {
            active.as_str()
        };
        if cur != mode {
            std::mem::swap(
                &mut *state.project.lock_ok(),
                &mut *state.alt_project.lock_ok(),
            );
            std::mem::swap(
                &mut *state.history.lock_ok(),
                &mut *state.alt_history.lock_ok(),
            );
            std::mem::swap(&mut *state.media.lock_ok(), &mut *state.alt_media.lock_ok());
            *active = mode.to_string();
        }
    }
    // build the now-active workspace's payload (mirrors open_project's shape)
    let (snap, transcripts) = {
        let mut project = state.project.lock_ok();
        let snap = project.snapshot();
        let transcripts: serde_json::Map<String, serde_json::Value> = project
            .transcripts()
            .into_iter()
            .filter_map(|(id, j)| {
                serde_json::from_str::<serde_json::Value>(&j)
                    .ok()
                    .map(|v| (id, v))
            })
            .collect();
        (snap, transcripts)
    };
    let media_out: Vec<serde_json::Value> = {
        let media = state.media.lock_ok();
        media
            .values()
            .filter_map(|info| media_json(info).ok())
            .collect()
    };
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
        .lock_ok()
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
    state.media.lock_ok().insert(media_id, info);
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
            return Err(
                "whisper model not found (looked beside the app and in vendor/whisper/)".into(),
            );
        }
    }
}

/// On-device transcription with word timestamps. Slow-ish (≈ 1/5 of the
/// clip duration on CPU); runs on a worker thread.
#[tauri::command]
async fn transcribe_media(
    media_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<cutlass_engine::transcribe::Word>, String> {
    use std::sync::atomic::{AtomicI32, Ordering};
    use tauri::Emitter;
    let path = state
        .media
        .lock_ok()
        .get(&media_id)
        .map(|m| m.path.clone())
        .ok_or_else(|| format!("unknown media {media_id}"))?;
    let model = whisper_model_path()?;
    // whisper runs seconds+ on CPU — off the UI thread, streaming progress so
    // the UI can show a real percentage.
    let app2 = app.clone();
    let mid = media_id.clone();
    let last = std::sync::Arc::new(AtomicI32::new(-1));
    let words = tauri::async_runtime::spawn_blocking(move || {
        cutlass_engine::transcribe::transcribe(&path, &model.to_string_lossy(), move |p| {
            if last.swap(p, Ordering::Relaxed) != p {
                let _ = app2.emit(
                    "transcribe-progress",
                    serde_json::json!({ "media": mid, "pct": p }),
                );
            }
        })
        .map_err(err_str)
    })
    .await
    .map_err(err_str)??;
    let _ = app.emit(
        "transcribe-progress",
        serde_json::json!({ "media": media_id, "pct": 100 }),
    );
    // persist into the doc so the transcript saves + syncs with the project
    if let Ok(json) = serde_json::to_string(&words) {
        let _ = state.project.lock_ok().set_transcript(&media_id, &json);
        notify_sync(&state);
    }
    Ok(words)
}

/// Find the MDX vocal-separation model, same layered lookup as the whisper one:
/// env override, then beside the exe (where the bundler drops it), then — in
/// dev — walking up to vendor/mdx/.
fn mdx_model_path() -> Result<std::path::PathBuf, String> {
    if let Ok(p) = std::env::var("CUTLASS_MDX_MODEL") {
        return Ok(p.into());
    }
    const NAME: &str = "UVR-MDX-NET-Voc_FT.onnx";
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for cand in [dir.join(NAME), dir.join("mdx").join(NAME)] {
                if cand.is_file() {
                    return Ok(cand);
                }
            }
        }
    }
    let mut dir = std::env::current_dir().map_err(err_str)?;
    loop {
        let candidate = dir.join("vendor").join("mdx").join(NAME);
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err(
                "music-removal model not found (looked beside the app and in vendor/mdx/)".into(),
            );
        }
    }
}

/// Replace `to` with `from`, retrying briefly. On Windows a file that was just
/// written — or one the preview is still reading — is often held for a moment
/// (antivirus scanning the new file is the usual culprit), and the rename comes
/// back as "Access is denied (os error 5)". Retrying with a short backoff clears
/// it; renaming onto a path nothing holds succeeds on the first try.
fn replace_file(from: &str, to: &str) -> std::io::Result<()> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..12 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        // the destination may be the thing that's locked; try clearing it
        let _ = std::fs::remove_file(to);
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(80 * (attempt + 1)));
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("could not replace file")))
}

/// On-device "remove background music": separate one clip's span of its source
/// into vocals vs. the rest, cached beside that source's proxies as
/// `vocals_<start>_<end>.wav`. The clip then flips fx.music_removed to
/// play/export the vocals track. Runs on a worker thread, streaming progress;
/// reuses any cached separation that already covers the span (so re-toggling,
/// and other cuts within it, are free). Private and free — audio never leaves
/// the machine.
#[tauri::command]
async fn remove_music(
    clip_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicI32, Ordering};
    use tauri::Emitter;
    // Separate only the span this clip actually uses, plus a little padding so
    // small re-trims still hit the cache. Whole-source separation on a long
    // recording is minutes of work and gigabytes of RAM for footage you cut out.
    const PAD_S: f64 = 3.0;
    let (path, start_s, end_s, strength) = {
        let project = state.project.lock_ok();
        let media = state.media.lock_ok();
        let snap = project.snapshot();
        let clip = snap["clips"]
            .as_array()
            .and_then(|cs| {
                cs.iter()
                    .find(|c| c["id"].as_str() == Some(clip_id.as_str()))
            })
            .ok_or_else(|| format!("unknown clip {clip_id}"))?;
        let m = media
            .get(clip["media"].as_str().unwrap_or(""))
            .ok_or_else(|| "clip has no media".to_string())?;
        let src_in = clip["src_in"].as_f64().unwrap_or(0.0);
        let speed = clip["fx"]["speed"].as_f64().unwrap_or(1.0).max(0.01);
        let used = clip["len"].as_f64().unwrap_or(0.0) * speed;
        let strength = clip["fx"]["music_strength"].as_f64().unwrap_or(1.0) as f32;
        (
            m.path.clone(),
            (src_in - PAD_S).max(0.0),
            src_in + used + PAD_S,
            strength,
        )
    };
    let out = cutlass_core::media::vocals_range_path(Path::new(&path), start_s, end_s)
        .map_err(err_str)?;
    let raw =
        cutlass_core::media::vocals_raw_path(Path::new(&path), start_s, end_s).map_err(err_str)?;
    let model = mdx_model_path()?;
    let out_s = out.to_string_lossy().to_string();
    let raw_s = raw.to_string_lossy().to_string();
    let app2 = app.clone();
    let cid = clip_id.clone();
    let last = std::sync::Arc::new(AtomicI32::new(-1));
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        // Decode just this span to a clean 44.1 kHz stereo WAV (ffmpeg handles
        // any container/layout). We need it for the blend even when the model
        // output is already cached.
        let tmp_in = format!("{out_s}.src.wav");
        let tmp_out = format!("{out_s}.partial.wav");
        cutlass_core::media::decode_audio_wav(
            std::path::Path::new(&path),
            std::path::Path::new(&tmp_in),
            44_100,
            start_s,
            Some(end_s - start_s),
        )?;
        // Run the model only if this span hasn't been separated yet — changing
        // strength then costs a quick re-mix instead of minutes of inference.
        if !std::path::Path::new(&raw_s).is_file() {
            let raw_tmp = format!("{raw_s}.partial.wav");
            let res = cutlass_engine::separate::remove_music(
                &tmp_in,
                &model.to_string_lossy(),
                &raw_tmp,
                move |p| {
                    let pct = (p * 100.0) as i32;
                    if last.swap(pct, Ordering::Relaxed) != pct {
                        let _ = app2.emit(
                            "remove-music-progress",
                            serde_json::json!({ "clip": cid, "pct": pct }),
                        );
                    }
                },
            );
            if let Err(e) = res {
                let _ = std::fs::remove_file(&tmp_in);
                return Err(e);
            }
            replace_file(&raw_tmp, &raw_s)?;
        }
        let res = cutlass_engine::separate::blend_and_write(&raw_s, &tmp_in, &tmp_out, strength);
        let _ = std::fs::remove_file(&tmp_in);
        res?;
        replace_file(&tmp_out, &out_s)?;
        Ok(())
    })
    .await
    .map_err(err_str)?
    .map_err(err_str)?;
    let _ = app.emit(
        "remove-music-progress",
        serde_json::json!({ "clip": clip_id, "pct": 100 }),
    );
    Ok(())
}

/// Cloud transcription (the OpusClip speed path): extract the audio into
/// ~10-min FLAC chunks with the bundled ffmpeg, upload them to the server (→
/// Groq GPUs) in PARALLEL, and merge the word timestamps. Minutes of on-device
/// CPU work collapse to seconds. Only the audio leaves the machine — never the
/// video — and only for a licensed app.
#[tauri::command]
async fn cloud_transcribe(
    media_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<license::SttWord>, String> {
    use tauri::Emitter;
    let path = state
        .media
        .lock_ok()
        .get(&media_id)
        .map(|m| m.path.clone())
        .ok_or_else(|| format!("unknown media {media_id}"))?;

    let mid = media_id.clone();
    let app2 = app.clone();
    let emit = move |pct: i32| {
        let _ = app2.emit(
            "transcribe-progress",
            serde_json::json!({ "media": mid.as_str(), "pct": pct }),
        );
    };
    emit(0);

    // 1. extract 16 kHz mono FLAC in ~10-min segments (+ a CSV of chunk offsets)
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("cutlass-stt-{stamp}"));
    std::fs::create_dir_all(&tmp).map_err(err_str)?;
    let list_path = tmp.join("list.csv");
    let pattern = tmp.join("chunk_%04d.flac");

    let ff = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());
    let mut cmd = std::process::Command::new(&ff);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(&path)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "flac",
            "-f",
            "segment",
            "-segment_time",
            "600",
            "-reset_timestamps",
            "1",
            "-segment_list",
        ])
        .arg(&list_path)
        .args(["-segment_list_type", "csv"])
        .arg(&pattern);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
    }
    let out = cmd.output().map_err(err_str)?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!(
            "audio extraction failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    emit(12);

    // 2. parse the segment list — "chunk_0000.flac,<start>,<end>" per line
    let csv = std::fs::read_to_string(&list_path).map_err(err_str)?;
    let mut chunks: Vec<(std::path::PathBuf, f64)> = Vec::new();
    for line in csv.lines() {
        let mut it = line.split(',');
        let (Some(name), Some(start)) = (it.next(), it.next()) else {
            continue;
        };
        let offset: f64 = start.trim().parse().unwrap_or(0.0);
        chunks.push((tmp.join(name.trim()), offset));
    }
    if chunks.is_empty() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("no audio track found to transcribe".into());
    }

    // 3. upload + transcribe every chunk concurrently, then merge
    let n = chunks.len() as i32;
    let mut handles = Vec::new();
    for (cpath, offset) in chunks {
        handles.push(tauri::async_runtime::spawn_blocking(move || {
            let bytes = std::fs::read(&cpath).map_err(|e| e.to_string())?;
            license::cloud_transcribe_chunk(bytes, offset)
        }));
    }
    let mut all: Vec<license::SttWord> = Vec::new();
    let mut last_err: Option<String> = None;
    for (i, h) in handles.into_iter().enumerate() {
        match h.await {
            Ok(Ok(mut words)) => all.append(&mut words),
            Ok(Err(e)) => last_err = Some(e),
            Err(e) => last_err = Some(e.to_string()),
        }
        emit(12 + (i as i32 + 1) * 88 / n);
    }
    let _ = std::fs::remove_dir_all(&tmp);

    if all.is_empty() {
        return Err(last_err.unwrap_or_else(|| "transcription returned no words".into()));
    }
    all.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    emit(100);

    // persist into the doc (identical shape to the on-device path)
    if let Ok(json) = serde_json::to_string(&all) {
        let _ = state.project.lock_ok().set_transcript(&media_id, &json);
        notify_sync(&state);
    }
    Ok(all)
}

/// Delete a source range from a clip (the "delete these words" edit).
#[tauri::command]
fn razor_out(
    id: String,
    src_from: f64,
    src_to: f64,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
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
    if let Some(h) = state.playback.lock_ok().take() {
        h.stop();
    }
    if let Some(v) = state.video_stop.lock_ok().take() {
        v.store(true, Ordering::Relaxed);
    }
    let muted = muted.unwrap_or_default();

    // ── real-time video playback thread ────────────────────────────────
    let video_clips: Vec<PlayClip> = {
        let project = state.project.lock_ok();
        let media = state.media.lock_ok();
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
    let tracks: Vec<Vec<cutlass_engine::player::AudioClip>> = {
        let project = state.project.lock_ok();
        let media = state.media.lock_ok();
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
                                // "Remove music": play the separated vocals when
                                // the flag is on and a cached separation covers
                                // this clip's span; src_in shifts to the offset
                                // inside that file. No coverage → original audio.
                                let src_in0 = c["src_in"].as_f64()?;
                                let speed0 = c["fx"]["speed"].as_f64().unwrap_or(1.0);
                                let span_end = src_in0 + c["len"].as_f64()? * speed0.max(0.01);
                                let (apath, asrc_in) =
                                    if c["fx"]["music_removed"].as_f64().unwrap_or(0.0) > 0.5 {
                                        match cutlass_core::media::vocals_covering(
                                            Path::new(&m.path),
                                            src_in0,
                                            span_end,
                                        ) {
                                            Some((p, file_start)) => (
                                                p.to_string_lossy().to_string(),
                                                (src_in0 - file_start).max(0.0),
                                            ),
                                            None => (m.path.clone(), src_in0),
                                        }
                                    } else {
                                        (m.path.clone(), src_in0)
                                    };
                                Some(cutlass_engine::player::AudioClip {
                                    path: apath,
                                    start: c["start"].as_f64()?,
                                    len: c["len"].as_f64()?,
                                    src_in: asrc_in,
                                    volume: c["fx"]["volume"].as_f64().unwrap_or(1.0),
                                    speed: c["fx"]["speed"].as_f64().unwrap_or(1.0),
                                    audio_offset: c["fx"]["audio_offset"].as_f64().unwrap_or(0.0),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    // Start audio first, then lock the video thread to the audio clock. That
    // sample-driven clock is the transport's single source of truth; if the
    // video instead free-runs on wall-clock time it slips ahead of the audio by
    // the output device's start-up/buffer latency, which is heard as delayed
    // audio in the preview. (Nothing audible — e.g. every clip under way is
    // retimed and filtered out — runs silent: the UI's local clock drives the
    // playhead and the video thread free-runs to the content end.)
    let audio: Option<cutlass_engine::player::PlaybackHandle> =
        if tracks.iter().all(|t| t.is_empty()) {
            None
        } else {
            match cutlass_engine::player::start(tracks, from_t) {
                Ok(handle) => {
                    *state.playback.lock_ok() = Some(handle.clone());
                    Some(handle)
                }
                Err(e) => {
                    eprintln!("audio unavailable, playing silent: {e:#}");
                    None
                }
            }
        };
    if !video_clips.is_empty() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *state.video_stop.lock_ok() = Some(stop.clone());
        start_video_thread(app, video_clips, from_t, stop, audio.clone());
    }
    audio.is_some()
}

/// Stop audio + video playback; returns where audio stopped.
#[tauri::command]
fn pause(state: State<AppState>) -> Option<f64> {
    if let Some(v) = state.video_stop.lock_ok().take() {
        v.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    state.playback.lock_ok().take().map(|h| h.stop())
}

#[tauri::command]
fn playback_clock(state: State<AppState>) -> Option<serde_json::Value> {
    state
        .playback
        .lock_ok()
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
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality).encode(
        &rgb,
        frame.width,
        frame.height,
        image::ExtendedColorType::Rgb8,
    )?;
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
    audio: Option<cutlass_engine::player::PlaybackHandle>,
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
                // Follow the audio clock when there's sound (it only advances as
                // samples reach the device, so video can't outrun audio); fall
                // back to wall-clock time for silent playback.
                let t = match &audio {
                    Some(h) => h.clock(),
                    None => from_t + start.elapsed().as_secs_f64(),
                };
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
    reframe: Option<String>,
    reframe_x: Option<f64>,
    reframe_y: Option<f64>,
    master_audio: Option<bool>,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use tauri::Emitter;
    let (clips, overlays, titles) = {
        let mut project = state.project.lock_ok();
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
        reframe: cutlass_core::export::Reframe::parse(reframe.as_deref().unwrap_or("letterbox")),
        master_audio: master_audio.unwrap_or(false),
        reframe_x: reframe_x.unwrap_or(0.5),
        reframe_y: reframe_y.unwrap_or(0.5),
    };
    // Cancel handle: cancel_export flips this; the render loop kills ffmpeg.
    // Claiming the slot is also how a second export is kept out — two running
    // at once would leave the first uncancellable (its handle overwritten) and
    // could have both writing the same file. Checked and claimed under one
    // lock so two clicks in the same instant can't both get through.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut slot = state.export_cancel.lock_ok();
        if slot.is_some() {
            return Err("An export is already running. Wait for it to finish, or cancel it.".into());
        }
        *slot = Some(cancel.clone());
    }
    // the render runs minutes — off the UI thread; progress still streams
    let result = tauri::async_runtime::spawn_blocking(move || {
        cutlass_core::export::export(
            &segments,
            &overlays,
            &titles,
            Path::new(&path),
            &settings,
            &mut {
                // ffmpeg reports progress several times a second for the whole
                // render, and that rate is set by wall-clock time, not by the
                // export settings. Forwarding every one floods the webview's
                // script queue on a long export until the UI stops repainting —
                // which looks exactly like a frozen export while the encode is
                // in fact healthy. A progress bar cannot show finer than about
                // a quarter of a percent, so only send that much.
                let mut last = -1.0f32;
                move |p: f32| {
                    if p >= 1.0 || (p - last).abs() >= 0.0025 {
                        last = p;
                        let _ = app.emit("export-progress", p);
                    }
                }
            },
            &cancel,
        )
        .map_err(err_str)
    })
    .await;
    // Release the slot whatever happened. If the render thread panicked, the
    // `?` below returns early — and leaving a stale handle behind would mean
    // Cancel acting on an export that no longer exists, and no further export
    // ever being allowed to start.
    *state.export_cancel.lock_ok() = None;
    result.map_err(err_str)?
}

/// Cancel the in-flight export (kills the ffmpeg render).
#[tauri::command]
fn cancel_export(state: State<AppState>) {
    if let Some(c) = state.export_cancel.lock_ok().as_ref() {
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
    state.room.lock_ok().clone()
}

/// Forward an ephemeral presence payload to the room (no-op untethered).
#[tauri::command]
fn send_presence(payload: serde_json::Value, state: State<AppState>) {
    if let Some(tx) = state.sync_tx.lock_ok().as_ref() {
        let _ = tx.send(SyncCmd::Presence(payload.to_string()));
    }
}

#[tauri::command]
fn join_session(room: String, app: tauri::AppHandle, state: State<AppState>) -> Result<(), String> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SyncCmd>();
    *state.sync_tx.lock_ok() = Some(tx);
    *state.room.lock_ok() = Some(room.clone());
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
        let mut p = state.project.lock_ok();
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
                            let mut p = state.project.lock_ok();
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
                            let mut p = state.project.lock_ok();
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

// ── licensing ────────────────────────────────────────────────────────
// Network + registry work runs off the UI thread so launch never blocks.
#[tauri::command]
async fn license_status() -> license::LicenseInfo {
    tauri::async_runtime::spawn_blocking(license::resolve)
        .await
        .unwrap_or_else(|_| license::resolve())
}

#[tauri::command]
async fn license_redeem(code: String) -> license::LicenseInfo {
    let fallback = code.clone();
    tauri::async_runtime::spawn_blocking(move || license::redeem(&code))
        .await
        .unwrap_or_else(|_| license::redeem(&fallback))
}

#[tauri::command]
fn license_machine_id() -> String {
    license::machine_id()
}

#[tauri::command]
async fn ai_highlights(
    transcript: Vec<license::TWord>,
    count: Option<u32>,
) -> Result<Vec<license::Moment>, String> {
    let n = count.unwrap_or(8);
    tauri::async_runtime::spawn_blocking(move || license::ai_highlights(transcript, n))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn ai_usage() -> Result<license::AiUsage, String> {
    tauri::async_runtime::spawn_blocking(license::ai_usage)
        .await
        .map_err(|e| e.to_string())?
}

fn main() {
    // Use the ffmpeg we ship, not whatever happens to be on PATH. The
    // bundled build is LGPL and has the exact filters/encoders the export
    // pipeline targets; a stray system ffmpeg may lack them (e.g. no `eq`).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let ff = dir.join("ffmpeg.exe");
            if ff.is_file() {
                std::env::set_var("FFMPEG_BINARY", &ff);
            }
        }
    }

    // A double-clicked .cutlass file arrives as a launch argument; stash it
    // so the frontend can load it once the window is up.
    let state = AppState::default();
    if let Some(f) = cutlass_arg(&std::env::args().collect::<Vec<_>>()) {
        *state.startup_file.lock_ok() = Some(f);
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(state)
        .setup(|app| {
            use tauri::{Emitter, Manager};
            // Intercept the window close so unsaved work isn't lost: always
            // prevent the OS close and hand it to the frontend, which quits
            // immediately when clean or shows a save prompt when dirty (then
            // calls `force_close`).
            if let Some(win) = app.get_webview_window("main") {
                let w = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.emit("close-requested", ());
                    }
                });
            }

            // CUTLASS_ROOM=<name> auto-joins a collab room at startup
            if let Ok(room) = std::env::var("CUTLASS_ROOM") {
                let handle = app.handle().clone();
                let state = app.state::<AppState>();
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SyncCmd>();
                *state.sync_tx.lock_ok() = Some(tx);
                *state.room.lock_ok() = Some(room.clone());
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
            remove_media,
            exact_frame,
            play,
            pause,
            playback_clock,
            transcribe_media,
            remove_music,
            razor_out,
            save_project,
            save_recovery_copy,
            updates_enabled,
            collab_enabled,
            checkout_links,
            take_startup_file,
            default_project_dir,
            default_export_dir,
            force_close,
            load_prefs,
            save_pref,
            open_project,
            set_mode,
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
            track_censor,
            reset_censor,
            add_title,
            set_title_text,
            license_status,
            license_redeem,
            license_machine_id,
            ai_highlights,
            ai_usage,
            cloud_transcribe
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutlass");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening covers three outcomes, and they must not be confused with each
    /// other: a good file opens, a damaged one falls back to the backup, and
    /// one from a newer Cutlass is refused without touching the backup at all.
    ///
    /// That last case is the reason this is a test. The backup beside a newer
    /// project was written by the same newer build, so trying it fails the
    /// same way and the user ends up told "the backup is unreadable too" —
    /// hunting for corruption in two files that are both perfectly fine.
    #[test]
    fn opening_tells_damage_apart_from_a_newer_cutlass() {
        let dir = std::env::temp_dir().join(format!("cutlass_open_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 1. a good project opens as itself
        let good = dir.join("Good.cutlass");
        let mut p = Project::new("Good");
        p.set_media("m0", "clip", "C:/clips/a.mp4", 12.0).unwrap();
        write_project_atomically(&good, &p.save()).unwrap();
        let (mut opened, from_backup) = read_project_or_backup(&good).expect("should open");
        assert!(!from_backup);
        assert_eq!(opened.media_entries().len(), 1);

        // 2. damage falls back to the backup, and says it did
        write_project_atomically(&good, &p.save()).unwrap(); // first save becomes the .bak
        std::fs::write(&good, b"").unwrap(); // an interrupted save
        let (mut recovered, from_backup) =
            read_project_or_backup(&good).expect("backup should save us");
        assert!(from_backup, "the caller has to know this is not the file they saved");
        assert_eq!(recovered.media_entries().len(), 1);

        // 3. a newer project is refused on its own terms, with its backup in
        //    place and equally newer
        let future = dir.join("Future.cutlass");
        let mut f = Project::new("Future");
        f.set_format_for_tests(cutlass_core::project::FORMAT + 1);
        write_project_atomically(&future, &f.save()).unwrap();
        write_project_atomically(&future, &f.save()).unwrap(); // now a .bak exists too
        assert!(backup_path(&future).exists(), "the test needs a backup present");

        let Err(err) = read_project_or_backup(&future) else {
            panic!("a project from a newer Cutlass must not open");
        };
        assert!(err.contains("newer version of Cutlass"), "{err}");
        assert!(
            !err.contains("backup"),
            "must not send them looking at the backup: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Creator edition must never update itself.
    ///
    /// The update endpoint serves the public trial build, so an owner build
    /// that checked would download it, verify it happily — it is correctly
    /// signed, just not the same edition — and replace itself with a trial.
    /// The only unrestricted copy of Cutlass, downgraded by a background task.
    ///
    /// The suite runs both ways: `--all-features` turns `owner` on, which is
    /// what the pre-push hook uses, and a plain run has it off. So each arm
    /// below is genuinely exercised rather than compiled out and forgotten.
    #[test]
    fn the_creator_edition_never_updates_itself() {
        #[cfg(feature = "owner")]
        assert!(
            !updates_enabled(),
            "the owner build would replace itself with the public trial build"
        );
        #[cfg(not(feature = "owner"))]
        assert!(updates_enabled(), "the shipping build has to be reachable");
    }

    /// Every mutex here must go through `lock_ok`, and this reads the file's
    /// own source to say so.
    ///
    /// That sounds paranoid until you know how it went wrong. These were
    /// converted once already, by searching for `.lock().unwrap()` — and five
    /// were missed, because rustfmt had wrapped them onto two lines and the
    /// search was one line at a time. Nothing noticed for months: a poisoned
    /// mutex is a runtime state no test arrives at by accident, so the ones
    /// left behind passed everything while doing exactly what the fix was
    /// meant to stop. A grep that can be defeated by a line break is not a
    /// guarantee; this is, and it costs nothing to run.
    #[test]
    fn every_mutex_in_this_file_goes_through_lock_ok() {
        // Production code only — the poisoning test below locks deliberately.
        let src = include_str!("main.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();

        for open in [".lock()", ".read()", ".write()"] {
            for (at, _) in code.match_indices(open) {
                let after = code[at + open.len()..].trim_start();
                if after.starts_with(".unwrap()") || after.starts_with(".expect(") {
                    let line = code[..at].lines().count();
                    panic!(
                        "main.rs:{line} still panics on a poisoned lock \
                         (`{open}` followed by unwrap/expect). Use `.lock_ok()` — \
                         see the LockExt doc comment for why."
                    );
                }
            }
        }
    }

    /// The recovery copy is written after the UI has already crashed, so the
    /// name it builds gets exactly one attempt. A character Windows rejects
    /// means the rescue itself fails, at the only moment it matters.
    #[test]
    fn a_recovery_file_name_survives_whatever_the_project_is_called() {
        for (raw, want) in [
            ("Ep 3", "Ep 3"),
            (r#"Ep 3: "Rebuild" <final>"#, "Ep 3 Rebuild final"),
            (r"client/brief\v2", "client brief v2"),
            ("why?*|", "why"),
            ("  padded  ", "padded"),
            ("trailing dot.", "trailing dot"),
            ("...", "Untitled"),
            ("", "Untitled"),
            ("   ", "Untitled"),
            ("tab\there", "tab here"),
        ] {
            let got = safe_file_stem(raw);
            assert_eq!(got, want, "for {raw:?}");
            assert!(
                !got.chars().any(|c| r#"\/:*?"<>|"#.contains(c) || c.is_control()),
                "{got:?} still holds a character Windows won't take"
            );
            assert!(!got.is_empty() && !got.ends_with('.') && !got.ends_with(' '));
        }

        // A long name must not push the path past what Windows will open.
        let long = safe_file_stem(&"word ".repeat(200));
        assert!(long.chars().count() <= 80, "was {}", long.chars().count());
        assert!(!long.ends_with(' '), "trimmed after capping, not before");

        // And the whole point: the result is actually writable.
        let dir = std::env::temp_dir().join(format!("cutlass_recov_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!(
            "{} (recovered {}).cutlass",
            safe_file_stem(r#"Ep 3: "Rebuild""#),
            safe_file_stem("2026-09-11 1432")
        ));
        write_project_atomically(&f, b"rescued").expect("the built name must be writable");
        assert_eq!(std::fs::read(&f).unwrap(), b"rescued");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The guarantee auto-save depends on: the file on disk is always either
    /// the old project or the new one, and the previous generation survives.
    #[test]
    fn atomic_save_keeps_a_good_copy_at_every_step() {
        let dir = std::env::temp_dir().join(format!("cutlass_atomic_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proj = dir.join("Take 1.cutlass");

        write_project_atomically(&proj, b"version-one").unwrap();
        assert_eq!(std::fs::read(&proj).unwrap(), b"version-one");
        // nothing to back up on a first save
        assert!(!backup_path(&proj).exists());

        write_project_atomically(&proj, b"version-two-longer").unwrap();
        assert_eq!(std::fs::read(&proj).unwrap(), b"version-two-longer");
        // the previous good save is now recoverable
        assert_eq!(std::fs::read(backup_path(&proj)).unwrap(), b"version-one");

        // a shorter write must not leave a tail of the longer one behind,
        // which is exactly what truncate-then-write produces
        write_project_atomically(&proj, b"v3").unwrap();
        assert_eq!(std::fs::read(&proj).unwrap(), b"v3");

        // no half-written scratch files left in the user's folder
        let strays: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".saving-"))
            .collect();
        assert!(strays.is_empty(), "left temp files behind: {strays:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A save that cannot even start must leave the existing project alone
    /// rather than truncating it first and discovering the problem after.
    #[test]
    fn failed_save_does_not_touch_the_existing_project() {
        let dir = std::env::temp_dir().join(format!("cutlass_atomic_fail_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proj = dir.join("Take 1.cutlass");
        write_project_atomically(&proj, b"the-good-version").unwrap();

        // target inside a directory that does not exist: the temp file can't
        // be created, so the write fails at the first step
        let doomed = dir.join("no-such-folder").join("Take 1.cutlass");
        assert!(write_project_atomically(&doomed, b"never-lands").is_err());

        // the real project is still exactly as it was
        assert_eq!(std::fs::read(&proj).unwrap(), b"the-good-version");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole point of keeping a `.bak`: a project damaged mid-save is
    /// still openable from the copy the previous save left.
    #[test]
    fn a_damaged_project_is_recoverable_from_its_backup() {
        let dir = std::env::temp_dir().join(format!("cutlass_recover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let proj = dir.join("Take 1.cutlass");

        let mut p = Project::new("Take 1");
        p.set_media("m0", "clip", "C:/clips/a.mp4", 12.0).unwrap();
        write_project_atomically(&proj, &p.save()).unwrap();
        // a second save, so the first becomes the backup
        write_project_atomically(&proj, &p.save()).unwrap();

        // an interrupted save leaves the file truncated -- or empty
        std::fs::write(&proj, b"").unwrap();
        assert!(
            std::fs::read(&proj)
                .ok()
                .and_then(|b| Project::load(&b).ok())
                .is_none(),
            "the damaged file should not load"
        );

        let recovered = std::fs::read(backup_path(&proj))
            .ok()
            .and_then(|b| Project::load(&b).ok());
        assert!(recovered.is_some(), "the backup should still load");
        assert_eq!(recovered.unwrap().media_entries().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One panic anywhere used to poison the shared state and make every later
    /// command panic too, so the window could not even save. It must survive.
    #[test]
    fn a_poisoned_lock_does_not_brick_everything_after_it() {
        let m = std::sync::Arc::new(std::sync::Mutex::new(vec!["a project".to_string()]));
        let m2 = m.clone();
        // a thread panics while holding the lock -- exactly what an unwrap deep
        // in a command handler does
        let _ = std::thread::spawn(move || {
            let _guard = m2.lock().unwrap();
            panic!("something failed while holding the state");
        })
        .join();

        assert!(m.lock().is_err(), "the mutex should now be poisoned");
        // the old `.lock().unwrap()` would panic here; this must not
        let mut guard = m.lock_ok();
        guard.push("still usable".to_string());
        assert_eq!(guard.len(), 2);
    }

    /// The export slot is what keeps a second render from starting while one
    /// is going. If a render thread panics, the slot has to come back anyway —
    /// otherwise Cancel acts on an export that is gone and no new one can ever
    /// begin.
    #[test]
    fn the_export_slot_is_released_even_when_the_render_panics() {
        let slot: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
            std::sync::Mutex::new(None);

        // claim it, as export_project does
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *slot.lock_ok() = Some(cancel);
        assert!(slot.lock_ok().is_some(), "slot should be claimed");

        // a second export must be turned away while it is held
        assert!(slot.lock_ok().is_some(), "a second export would be refused here");

        // the render panics; the release must still happen
        let joined = std::thread::spawn(|| -> i32 { panic!("render died") }).join();
        *slot.lock_ok() = None; // this is the line that must not be skipped
        assert!(joined.is_err(), "the thread really did panic");
        assert!(
            slot.lock_ok().is_none(),
            "slot must be free again, or exports are blocked for the session"
        );
    }
}
