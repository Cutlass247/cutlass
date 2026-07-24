//! Media import: probe + scrub-proxy generation.
//!
//! Per Spike B (spikes/media-engine/FINDINGS.md): proxy-first is the
//! baseline. On import we generate a low-res JPEG frame strip ("scrub
//! proxy") in one sequential ffmpeg pass; scrubbing then never touches
//! the source file. The in-process libav engine replaces this for
//! full-quality playback in a later milestone — this module is the
//! walking skeleton's decode path, and stays useful afterwards as the
//! thumbnail/filmstrip generator.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use ffmpeg_sidecar::command::FfmpegCommand;

/// Cap on scrub-proxy frames per clip; interval stretches with duration.
/// Pub so the engine-based import path in the app matches this layout.
pub const MAX_SCRUB_FRAMES: f64 = 240.0;
pub const SCRUB_WIDTH: u32 = 480;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub duration_s: f64,
    /// native source dimensions; 0 when unknown
    pub width: u32,
    pub height: u32,
    /// scrub-proxy sampling rate (frames per second of source time)
    pub scrub_fps: f64,
    /// scrub-proxy frames, in time order
    pub thumb_paths: Vec<PathBuf>,
    /// normalized audio peaks across the whole file (empty = no audio)
    pub waveform: Vec<f32>,
}

/// Peak waveform: ~1200 normalized buckets from 8 kHz mono PCM. Empty
/// for files without audio.
pub fn waveform(path: &Path) -> Vec<f32> {
    let tmp = std::env::temp_dir().join(format!("cutlass-wf-{:016x}.pcm", path_hash(path)));
    let ok = FfmpegCommand::new()
        .input(path.to_string_lossy())
        .args(["-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "-y"])
        .output(tmp.to_string_lossy())
        .spawn()
        .and_then(|mut c| c.wait())
        .map(|s| s.success())
        .unwrap_or(false);
    let bytes = if ok { std::fs::read(&tmp).unwrap_or_default() } else { Vec::new() };
    let _ = std::fs::remove_file(&tmp);
    let samples = bytes.len() / 2;
    if samples < 800 {
        return Vec::new();
    }
    let bucket = (samples / 1200).max(80);
    let mut peaks = Vec::with_capacity(samples / bucket + 1);
    let mut peak = 0i32;
    for (i, chunk) in bytes.chunks_exact(2).enumerate() {
        peak = peak.max((i16::from_le_bytes([chunk[0], chunk[1]]) as i32).abs());
        if (i + 1) % bucket == 0 {
            peaks.push(peak as f32);
            peak = 0;
        }
    }
    let max = peaks.iter().fold(1.0f32, |m, p| m.max(*p));
    peaks.iter().map(|p| p / max).collect()
}

pub fn ensure_ffmpeg() -> anyhow::Result<()> {
    // The app pins FFMPEG_BINARY to the ffmpeg it ships; when that's present
    // there's nothing to fetch (and we must not pull a different build —
    // ours is LGPL with the exact filters/encoders export targets).
    if let Ok(p) = std::env::var("FFMPEG_BINARY") {
        if Path::new(&p).is_file() {
            return Ok(());
        }
    }
    ffmpeg_sidecar::download::auto_download()
        .map_err(|e| anyhow::anyhow!("ffmpeg download failed: {e}"))
}

/// Parse "Duration: HH:MM:SS.cc" from ffmpeg -i stderr.
pub fn probe_duration_s(path: &Path) -> anyhow::Result<f64> {
    let mut child = FfmpegCommand::new()
        .input(path.to_string_lossy())
        .args(["-f", "null", "-t", "0.01", "-"])
        .spawn()?;
    let mut duration = None;
    for event in child.iter()? {
        if let ffmpeg_sidecar::event::FfmpegEvent::ParsedDuration(d) = event {
            duration = Some(d.duration);
        }
    }
    duration.ok_or_else(|| anyhow::anyhow!("could not probe duration of {}", path.display()))
}

/// Native pixel dimensions of the first video stream, (0, 0) when unknown.
/// The export UI uses this to stop people upscaling past their source —
/// rendering 1080p footage at 4K measurably *lowers* quality and triples
/// the file size.
pub fn probe_dimensions(path: &Path) -> (u32, u32) {
    let Ok(mut child) = FfmpegCommand::new()
        .input(path.to_string_lossy())
        .args(["-f", "null", "-t", "0.01", "-"])
        .spawn()
    else {
        return (0, 0);
    };
    let mut dims = (0, 0);
    if let Ok(events) = child.iter() {
        for event in events {
            if let ffmpeg_sidecar::event::FfmpegEvent::ParsedInputStream(s) = event {
                if let ffmpeg_sidecar::event::StreamTypeSpecificData::Video(v) =
                    &s.type_specific_data
                {
                    if dims == (0, 0) {
                        dims = (v.width, v.height);
                    }
                }
            }
        }
    }
    dims
}

/// Generate (or reuse) the scrub proxy for `path`. Returns frame files in
/// time order plus the sampling fps used.
pub fn scrub_proxy(path: &Path, duration_s: f64) -> anyhow::Result<(Vec<PathBuf>, f64)> {
    let fps = (MAX_SCRUB_FRAMES / duration_s.max(0.1)).min(10.0);
    let dir = cache_dir(path)?;
    let existing = read_frames(&dir)?;
    if !existing.is_empty() {
        return Ok((existing, fps));
    }
    let pattern = dir.join("f%05d.jpg");
    let status = FfmpegCommand::new()
        .input(path.to_string_lossy())
        .args([
            "-vf",
            &format!("fps={fps},scale={SCRUB_WIDTH}:-2"),
            "-q:v",
            "5",
            "-y",
            &pattern.to_string_lossy(),
        ])
        .spawn()?
        .wait()?;
    if !status.success() {
        anyhow::bail!("scrub proxy generation failed for {}", path.display());
    }
    let frames = read_frames(&dir)?;
    if frames.is_empty() {
        anyhow::bail!("no scrub frames produced for {}", path.display());
    }
    Ok((frames, fps))
}

pub fn import(path: &Path) -> anyhow::Result<MediaInfo> {
    let duration_s = probe_duration_s(path)?;
    let (width, height) = probe_dimensions(path);
    let (thumb_paths, scrub_fps) = scrub_proxy(path, duration_s)?;
    let waveform = waveform(path);
    Ok(MediaInfo {
        id: format!("m{:016x}", path_hash(path)),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "clip".into()),
        path: path.to_string_lossy().to_string(),
        duration_s,
        width,
        height,
        scrub_fps,
        thumb_paths,
        waveform,
    })
}

pub fn path_hash(path: &Path) -> u64 {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(meta) = std::fs::metadata(path) {
        meta.len().hash(&mut h);
    }
    h.finish()
}

pub fn cache_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join("cutlass-cache")
        .join(format!("{:016x}", path_hash(path)));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn read_frames(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut frames: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .collect();
    frames.sort();
    Ok(frames)
}
