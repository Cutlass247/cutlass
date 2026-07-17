//! Timeline → MP4 render, via the managed ffmpeg binary.
//!
//! Deliberate architecture: export is a BATCH job, so Spike B's
//! "no subprocess for interactive decode" rule doesn't apply — and
//! running GPL encoders in a separate process keeps the app's LGPL
//! linkage clean. Encoder: h264_qsv (Intel hw) first, libx264 fallback.

use std::path::Path;

use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;

#[derive(Debug, Clone)]
pub enum Segment {
    Clip { path: String, src_in: f64, len: f64 },
    Gap { len: f64 },
}

impl Segment {
    fn len(&self) -> f64 {
        match self {
            Segment::Clip { len, .. } | Segment::Gap { len } => *len,
        }
    }
}

/// A V2 clip composited over the program at its timeline position.
/// (Video only — matching in-app playback, where V1 carries the audio.)
#[derive(Debug, Clone)]
pub struct Overlay {
    pub path: String,
    pub src_in: f64,
    pub len: f64,
    pub start: f64,
}

pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, fps: 30 }
    }
}

/// Does the file have an audio stream? (stderr probe)
pub fn has_audio(path: &str) -> bool {
    let Ok(mut child) = FfmpegCommand::new()
        .input(path)
        .args(["-t", "0.01", "-f", "null", "-"])
        .spawn()
    else {
        return false;
    };
    let Ok(iter) = child.iter() else { return false };
    for event in iter {
        if let FfmpegEvent::ParsedInputStream(stream) = event {
            if stream.is_audio() {
                return true;
            }
        }
    }
    false
}

/// Render `segments` (the V1 program, in order) with `overlays` (V2)
/// composited on top. Returns the encoder used. `progress` gets 0..=1.
pub fn export(
    segments: &[Segment],
    overlays: &[Overlay],
    out: &Path,
    settings: &ExportSettings,
    progress: &mut dyn FnMut(f32),
) -> anyhow::Result<String> {
    anyhow::ensure!(!segments.is_empty(), "nothing to export");
    let total: f64 = segments.iter().map(|s| s.len()).sum();

    for encoder in ["h264_qsv", "libx264"] {
        progress(0.0);
        match run_export(segments, overlays, out, settings, encoder, total, progress) {
            Ok(()) => return Ok(encoder.to_string()),
            Err(e) if encoder == "h264_qsv" => {
                eprintln!("qsv encode failed ({e:#}); falling back to libx264");
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn run_export(
    segments: &[Segment],
    overlays: &[Overlay],
    out: &Path,
    s: &ExportSettings,
    encoder: &str,
    total: f64,
    progress: &mut dyn FnMut(f32),
) -> anyhow::Result<()> {
    let (w, h, fps) = (s.width, s.height, s.fps);
    let mut cmd = FfmpegCommand::new();
    let mut filters = String::new();
    let mut concat_inputs = String::new();
    let mut input_idx = 0u32;

    for (k, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Clip { path, src_in, len } => {
                // input-level seek: ffmpeg decodes from the prior keyframe
                // and discards up to the exact point — frame-accurate
                cmd.args(["-ss", &format!("{src_in:.3}"), "-t", &format!("{len:.3}")]);
                cmd.input(path.as_str());
                let vi = input_idx;
                input_idx += 1;
                filters.push_str(&format!(
                    "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                     pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},setpts=PTS-STARTPTS,\
                     format=yuv420p[v{k}];"
                ));
                if has_audio(path) {
                    filters.push_str(&format!(
                        "[{vi}:a]aresample=48000,asetpts=PTS-STARTPTS[a{k}];"
                    ));
                } else {
                    cmd.args(["-f", "lavfi", "-t", &format!("{len:.3}")]);
                    cmd.input("anullsrc=r=48000:cl=stereo");
                    filters.push_str(&format!("[{input_idx}:a]acopy[a{k}];"));
                    input_idx += 1;
                }
            }
            Segment::Gap { len } => {
                cmd.args(["-f", "lavfi", "-t", &format!("{len:.3}")]);
                cmd.input(format!("color=black:s={w}x{h}:r={fps}"));
                filters.push_str(&format!("[{input_idx}:v]format=yuv420p[v{k}];"));
                input_idx += 1;
                cmd.args(["-f", "lavfi", "-t", &format!("{len:.3}")]);
                cmd.input("anullsrc=r=48000:cl=stereo");
                filters.push_str(&format!("[{input_idx}:a]acopy[a{k}];"));
                input_idx += 1;
            }
        }
        concat_inputs.push_str(&format!("[v{k}][a{k}]"));
    }
    filters.push_str(&format!(
        "{concat_inputs}concat=n={}:v=1:a=1[catv][cata];",
        segments.len()
    ));

    // V2 overlays: video switches in over the program at its timeline
    // position; overlay audio is delayed to that position and mixed in
    let mut base = "catv".to_string();
    let mut overlay_audio: Vec<String> = Vec::new();
    for (j, ov) in overlays.iter().enumerate() {
        cmd.args(["-ss", &format!("{:.3}", ov.src_in), "-t", &format!("{:.3}", ov.len)]);
        cmd.input(ov.path.as_str());
        let vi = input_idx;
        input_idx += 1;
        let (t0, t1) = (ov.start, ov.start + ov.len);
        filters.push_str(&format!(
            "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},format=yuv420p,\
             setpts=PTS-STARTPTS+{t0:.3}/TB[ov{j}];\
             [{base}][ov{j}]overlay=eof_action=pass:enable='between(t,{t0:.3},{t1:.3})'[ovd{j}];"
        ));
        base = format!("ovd{j}");
        if has_audio(&ov.path) {
            let ms = (ov.start * 1000.0).round() as i64;
            filters.push_str(&format!(
                "[{vi}:a]aresample=48000,adelay={ms}|{ms}[oa{j}];"
            ));
            overlay_audio.push(format!("[oa{j}]"));
        }
    }
    if overlay_audio.is_empty() {
        filters.push_str("[cata]anull[outa];");
    } else {
        filters.push_str(&format!(
            "[cata]{}amix=inputs={}:duration=first:normalize=0[outa];",
            overlay_audio.join(""),
            overlay_audio.len() + 1
        ));
    }
    filters.push_str(&format!("[{base}]null[outv]"));

    let quality: &[&str] = if encoder == "h264_qsv" {
        &["-global_quality", "23"]
    } else {
        &["-crf", "20", "-preset", "fast"]
    };
    let mut child = cmd
        .args(["-filter_complex", &filters])
        .args(["-map", "[outv]", "-map", "[outa]"])
        .args(["-c:v", encoder])
        .args(quality)
        .args(["-c:a", "aac", "-b:a", "192k"])
        .args(["-movflags", "+faststart", "-y"])
        .output(out.to_string_lossy())
        .spawn()?;

    let mut saw_error = None;
    for event in child.iter()? {
        match event {
            FfmpegEvent::Progress(p) => {
                let done = parse_time_s(&p.time).unwrap_or(0.0);
                progress((done / total.max(0.001)).clamp(0.0, 1.0) as f32);
            }
            FfmpegEvent::Log(level, line)
                if format!("{level:?}").contains("Error") && saw_error.is_none() =>
            {
                saw_error = Some(line);
            }
            _ => {}
        }
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!(
            "ffmpeg exited with {status}: {}",
            saw_error.unwrap_or_default()
        );
    }
    progress(1.0);
    Ok(())
}

/// "HH:MM:SS.cc" → seconds
fn parse_time_s(t: &str) -> Option<f64> {
    let parts: Vec<&str> = t.split(':').collect();
    match parts.as_slice() {
        [h, m, sec] => Some(
            h.parse::<f64>().ok()? * 3600.0
                + m.parse::<f64>().ok()? * 60.0
                + sec.parse::<f64>().ok()?,
        ),
        [m, sec] => Some(m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?),
        [sec] => sec.parse().ok(),
        _ => None,
    }
}

/// Build the segment list for a track from (start, len, src_in, path)
/// tuples: sorts, inserts gaps, ignores overlaps (later start wins the
/// overlap region is v2 territory — v1 assumes non-overlapping V1).
pub fn segments_for_track(mut clips: Vec<(f64, f64, f64, String)>) -> Vec<Segment> {
    clips.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut segs = Vec::new();
    let mut t = 0.0f64;
    for (start, len, src_in, path) in clips {
        if start > t + 1e-3 {
            segs.push(Segment::Gap { len: start - t });
        }
        segs.push(Segment::Clip { path, src_in, len });
        t = start + len;
    }
    segs
}
