//! Timeline → MP4 render, via the managed ffmpeg binary.
//!
//! Deliberate architecture: export is a BATCH job, so Spike B's
//! "no subprocess for interactive decode" rule doesn't apply — and
//! running GPL encoders in a separate process keeps the app's LGPL
//! linkage clean. Encoder: h264_qsv (Intel hw) first, libx264 fallback.

use std::collections::BTreeMap;
use std::path::Path;

use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;

/// Per-clip Effect Controls resolved to identity-defaulted numbers. The
/// same vocabulary the preview (CSS) and playback (gain) speak.
#[derive(Debug, Clone)]
pub struct ClipFx {
    pub brightness: f64, // 0, range -1..1
    pub contrast: f64,   // 1
    pub saturation: f64, // 1
    pub scale: f64,      // 1
    pub rot: f64,        // degrees
    pub pos_x: f64,      // fraction of width
    pub pos_y: f64,      // fraction of height
    pub fade_in: f64,    // seconds
    pub fade_out: f64,   // seconds
    pub volume: f64,     // 1
    pub speed: f64,      // 1 (2 = 2× faster, 0.5 = slow-mo)
}

impl Default for ClipFx {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            scale: 1.0,
            rot: 0.0,
            pos_x: 0.0,
            pos_y: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            volume: 1.0,
            speed: 1.0,
        }
    }
}

/// atempo only accepts 0.5..=2.0 per instance; chain to reach any factor.
fn atempo_chain(speed: f64) -> String {
    let mut s = speed.clamp(0.1, 8.0);
    let mut parts = Vec::new();
    while s > 2.0 {
        parts.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s *= 2.0;
    }
    parts.push(format!("atempo={s:.4}"));
    parts.join(",")
}

impl ClipFx {
    fn set_param(&mut self, key: &str, v: f64) {
        match key {
            "brightness" => self.brightness = v,
            "contrast" => self.contrast = v,
            "saturation" => self.saturation = v,
            "scale" => self.scale = v,
            "rot" => self.rot = v,
            "pos_x" => self.pos_x = v,
            "pos_y" => self.pos_y = v,
            "speed" => self.speed = v,
            "fade_in" => self.fade_in = v,
            "fade_out" => self.fade_out = v,
            "volume" => self.volume = v,
            _ => {}
        }
    }

    /// This fx with each keyframed param overridden by its interpolated
    /// value at clip-relative time `t`.
    pub fn sampled_at(&self, kf: &BTreeMap<String, Vec<(f64, f64)>>, t: f64) -> Self {
        let mut out = self.clone();
        for (param, points) in kf {
            if let Some(v) = crate::project::interp(points, t) {
                out.set_param(param, v);
            }
        }
        out
    }

    pub fn from_map(m: &BTreeMap<String, f64>) -> Self {
        let d = Self::default();
        let g = |k: &str, def: f64| m.get(k).copied().unwrap_or(def);
        Self {
            brightness: g("brightness", d.brightness),
            contrast: g("contrast", d.contrast),
            saturation: g("saturation", d.saturation),
            scale: g("scale", d.scale),
            rot: g("rot", d.rot),
            pos_x: g("pos_x", d.pos_x),
            pos_y: g("pos_y", d.pos_y),
            fade_in: g("fade_in", d.fade_in),
            fade_out: g("fade_out", d.fade_out),
            volume: g("volume", d.volume),
            speed: {
                let s = g("speed", d.speed);
                if s > 0.05 { s } else { 1.0 }
            },
        }
    }
    fn has_transform(&self) -> bool {
        self.scale != 1.0 || self.rot != 0.0 || self.pos_x != 0.0 || self.pos_y != 0.0
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Clip { path: String, src_in: f64, len: f64, fx: ClipFx },
    Gap { len: f64 },
    /// A `dur`-second dissolve (or dip-to-black) from A's tail into B's
    /// head. Both windows are fitted then combined with xfade/acrossfade.
    Transition {
        a_path: String,
        a_src: f64,
        b_path: String,
        b_src: f64,
        dur: f64,
        dip: bool,
    },
}

impl Segment {
    fn len(&self) -> f64 {
        match self {
            Segment::Clip { len, .. } | Segment::Gap { len } => *len,
            Segment::Transition { dur, .. } => *dur,
        }
    }
}

/// One V1 clip as seen by the export segment builder, including any
/// transition INTO it from the previous adjacent clip.
#[derive(Debug, Clone)]
pub struct ExportClip {
    pub start: f64,
    pub len: f64,
    pub src_in: f64,
    pub path: String,
    pub fx: ClipFx,
    pub trans_dur: f64, // 0 = hard cut
    pub trans_dip: bool,
    /// param → sorted (clip-relative time, value); empty = no animation.
    pub kf: BTreeMap<String, Vec<(f64, f64)>>,
}

/// Keyframed clips render as piecewise-constant sub-segments. Cap keeps
/// the filtergraph bounded; step grows for long clips.
const KF_STEP_S: f64 = 0.2;
const KF_MAX_SEGS: usize = 80;

/// A clip from a track above the base, composited over the program at its
/// timeline position. Higher video tracks stack in order; `audio_only`
/// beds (from A-tracks) contribute their audio to the mix but no picture.
#[derive(Debug, Clone)]
pub struct Overlay {
    pub path: String,
    pub src_in: f64,
    pub len: f64,
    pub start: f64,
    pub fx: ClipFx,
    pub audio_only: bool,
}

/// A generated text title drawn over the program during its window.
#[derive(Debug, Clone)]
pub struct Title {
    pub text: String,
    pub start: f64,
    pub len: f64,
    pub pos_x: f64,    // fraction of width, 0 = centered
    pub pos_y: f64,    // fraction of height, 0 = centered
    pub font_size: f64, // px at 1080p, scaled to output height
    pub bg: f64,        // background band opacity, 0 = none
}

/// Escape text for ffmpeg drawtext `text='...'`. Apostrophes become
/// typographic ’ to sidestep single-quote termination entirely.
fn esc_drawtext(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('%', "\\%")
        .replace('\'', "\u{2019}")
        .replace('\n', " ")
        .replace('\r', "")
}

/// A ffmpeg-filter-safe path (forward slashes, escaped drive colon).
fn ff_path(p: &str) -> String {
    p.replace('\\', "/").replace(':', "\\:")
}

fn title_font() -> String {
    for f in [
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ] {
        if std::path::Path::new(f).exists() {
            return f.to_string();
        }
    }
    "C:/Windows/Fonts/arial.ttf".to_string()
}

/// Build a clip's fitted-frame video chain `[{vi}:v] → [v{k}]`:
/// letterbox to frame, color-correct, optional transform, optional fades.
fn clip_video_chain(vi: u32, k: usize, len: f64, w: u32, h: u32, fps: u32, fx: &ClipFx) -> String {
    // retime: source span was len*speed; /speed compresses it to len
    let setpts = if (fx.speed - 1.0).abs() < 1e-9 {
        "setpts=PTS-STARTPTS".to_string()
    } else {
        format!("setpts=(PTS-STARTPTS)/{:.5}", fx.speed)
    };
    let mut s = format!(
        "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},{setpts},\
         format=yuv420p,eq=brightness={b}:contrast={c}:saturation={sat}[cf{k}];",
        b = fx.brightness,
        c = fx.contrast,
        sat = fx.saturation
    );
    let mut cur = format!("cf{k}");
    if fx.has_transform() {
        let (px, py) = (fx.pos_x * w as f64, fx.pos_y * h as f64);
        s.push_str(&format!("[{cur}]scale=iw*{sc}:ih*{sc}[sc{k}];", sc = fx.scale));
        s.push_str(&format!(
            "color=c=black:s={w}x{h}:r={fps}:d={len:.3},format=yuv420p[bg{k}];"
        ));
        s.push_str(&format!(
            "[bg{k}][sc{k}]overlay=x=(W-w)/2+({px:.1}):y=(H-h)/2+({py:.1}):eof_action=pass[tf{k}];"
        ));
        cur = format!("tf{k}");
        if fx.rot != 0.0 {
            let rad = fx.rot * std::f64::consts::PI / 180.0;
            s.push_str(&format!("[{cur}]rotate={rad:.5}:fillcolor=black[rt{k}];"));
            cur = format!("rt{k}");
        }
    }
    if fx.fade_in > 0.0 || fx.fade_out > 0.0 {
        s.push_str(&format!("[{cur}]"));
        if fx.fade_in > 0.0 {
            s.push_str(&format!("fade=t=in:st=0:d={:.3}", fx.fade_in));
            if fx.fade_out > 0.0 {
                s.push(',');
            }
        }
        if fx.fade_out > 0.0 {
            s.push_str(&format!(
                "fade=t=out:st={:.3}:d={:.3}",
                (len - fx.fade_out).max(0.0),
                fx.fade_out
            ));
        }
        s.push_str(&format!("[v{k}];"));
    } else {
        s.push_str(&format!("[{cur}]null[v{k}];"));
    }
    s
}

/// Build a clip's audio chain `[{vi}:a] → [a{k}]`: resample, gain, fades.
fn clip_audio_chain(vi: u32, k: usize, len: f64, fx: &ClipFx) -> String {
    let mut c = format!("[{vi}:a]aresample=48000,asetpts=PTS-STARTPTS");
    if (fx.speed - 1.0).abs() >= 1e-9 {
        // atempo is pitch-corrected; input span len*speed → len
        c.push(',');
        c.push_str(&atempo_chain(fx.speed));
    }
    if fx.volume != 1.0 {
        c.push_str(&format!(",volume={:.4}", fx.volume));
    }
    if fx.fade_in > 0.0 {
        c.push_str(&format!(",afade=t=in:st=0:d={:.3}", fx.fade_in));
    }
    if fx.fade_out > 0.0 {
        c.push_str(&format!(
            ",afade=t=out:st={:.3}:d={:.3}",
            (len - fx.fade_out).max(0.0),
            fx.fade_out
        ));
    }
    c.push_str(&format!("[a{k}];"));
    c
}

/// Container + codec the export renders to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Mp4H264,   // universal
    Mp4H265,   // HEVC — smaller files
    MovProres, // editing/master quality
    WebmVp9,   // web
}

impl ExportFormat {
    pub fn parse(s: &str) -> Self {
        match s {
            "mp4_h265" | "h265" | "hevc" => Self::Mp4H265,
            "mov_prores" | "prores" => Self::MovProres,
            "webm_vp9" | "webm" | "vp9" => Self::WebmVp9,
            _ => Self::Mp4H264,
        }
    }
    /// File extension (also selects the container).
    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp4H264 | Self::Mp4H265 => "mp4",
            Self::MovProres => "mov",
            Self::WebmVp9 => "webm",
        }
    }
    /// Video encoders to try in order (first that works wins).
    fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::Mp4H264 => &["h264_qsv", "libx264"],
            Self::Mp4H265 => &["libx265"],
            Self::MovProres => &["prores_ks"],
            Self::WebmVp9 => &["libvpx-vp9"],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub fn parse(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "high" => Self::High,
            _ => Self::Medium,
        }
    }
}

pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: ExportFormat,
    pub quality: Quality,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            format: ExportFormat::Mp4H264,
            quality: Quality::Medium,
        }
    }
}

/// The output-encoding args for a format/quality/encoder combination —
/// everything after the filtergraph maps.
fn output_args(format: ExportFormat, quality: Quality, encoder: &str) -> Vec<String> {
    let s = |x: &str| x.to_string();
    let mut a = vec![s("-c:v"), s(encoder)];
    match format {
        ExportFormat::Mp4H264 => {
            if encoder == "h264_qsv" {
                let q = match quality {
                    Quality::Low => 28,
                    Quality::Medium => 23,
                    Quality::High => 20,
                };
                a.extend([s("-global_quality"), q.to_string()]);
            } else {
                let q = match quality {
                    Quality::Low => 28,
                    Quality::Medium => 23,
                    Quality::High => 18,
                };
                a.extend([s("-crf"), q.to_string(), s("-preset"), s("fast")]);
            }
            a.extend([s("-pix_fmt"), s("yuv420p")]);
            a.extend([s("-c:a"), s("aac"), s("-b:a"), s("192k")]);
            a.extend([s("-movflags"), s("+faststart")]);
        }
        ExportFormat::Mp4H265 => {
            let q = match quality {
                Quality::Low => 30,
                Quality::Medium => 26,
                Quality::High => 22,
            };
            a.extend([s("-crf"), q.to_string(), s("-preset"), s("fast")]);
            a.extend([s("-pix_fmt"), s("yuv420p")]);
            a.extend([s("-tag:v"), s("hvc1")]); // QuickTime-playable HEVC
            a.extend([s("-c:a"), s("aac"), s("-b:a"), s("192k")]);
            a.extend([s("-movflags"), s("+faststart")]);
        }
        ExportFormat::MovProres => {
            let p = match quality {
                Quality::Low => 1,    // ProRes 422 LT
                Quality::Medium => 2, // ProRes 422
                Quality::High => 3,   // ProRes 422 HQ
            };
            a.extend([s("-profile:v"), p.to_string()]);
            a.extend([s("-pix_fmt"), s("yuv422p10le")]);
            a.extend([s("-c:a"), s("pcm_s16le")]);
        }
        ExportFormat::WebmVp9 => {
            let q = match quality {
                Quality::Low => 36,
                Quality::Medium => 31,
                Quality::High => 24,
            };
            a.extend([s("-crf"), q.to_string(), s("-b:v"), s("0"), s("-row-mt"), s("1")]);
            a.extend([s("-pix_fmt"), s("yuv420p")]);
            a.extend([s("-c:a"), s("libopus"), s("-b:a"), s("160k")]);
        }
    }
    a
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
    titles: &[Title],
    out: &Path,
    settings: &ExportSettings,
    progress: &mut dyn FnMut(f32),
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<String> {
    use std::sync::atomic::Ordering;
    anyhow::ensure!(!segments.is_empty(), "nothing to export");
    let total: f64 = segments.iter().map(|s| s.len()).sum();

    let encoders = settings.format.encoders();
    for (i, encoder) in encoders.iter().enumerate() {
        progress(0.0);
        match run_export(segments, overlays, titles, out, settings, encoder, total, progress, cancel) {
            Ok(()) => return Ok(encoder.to_string()),
            Err(e) => {
                // a user cancel must not fall through to the next encoder
                if cancel.load(Ordering::Relaxed) {
                    return Err(e);
                }
                if i + 1 < encoders.len() {
                    eprintln!("{encoder} encode failed ({e:#}); trying {}", encoders[i + 1]);
                } else {
                    return Err(e);
                }
            }
        }
    }
    unreachable!()
}

fn run_export(
    segments: &[Segment],
    overlays: &[Overlay],
    titles: &[Title],
    out: &Path,
    s: &ExportSettings,
    encoder: &str,
    total: f64,
    progress: &mut dyn FnMut(f32),
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<()> {
    let (w, h, fps) = (s.width, s.height, s.fps);
    let mut cmd = FfmpegCommand::new();
    let mut filters = String::new();
    let mut concat_inputs = String::new();
    let mut input_idx = 0u32;

    for (k, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Clip { path, src_in, len, fx } => {
                // input-level seek: ffmpeg decodes from the prior keyframe
                // and discards up to the exact point — frame-accurate.
                // With speed, consume `len*speed` of source (retimed to
                // `len` on the timeline by setpts/atempo in the chains).
                let src_dur = len * fx.speed;
                cmd.args(["-ss", &format!("{src_in:.3}"), "-t", &format!("{src_dur:.3}")]);
                cmd.input(path.as_str());
                let vi = input_idx;
                input_idx += 1;
                filters.push_str(&clip_video_chain(vi, k, *len, w, h, fps, fx));
                if has_audio(path) {
                    filters.push_str(&clip_audio_chain(vi, k, *len, fx));
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
            Segment::Transition { a_path, a_src, b_path, b_src, dur, dip } => {
                let fit = |vi: u32, tag: &str| {
                    format!(
                        "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                         pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},setpts=PTS-STARTPTS,\
                         format=yuv420p[{tag}{k}];"
                    )
                };
                // A tail window (input `ai`) and B head window (input `bi`);
                // each input carries both :v and :a.
                cmd.args(["-ss", &format!("{a_src:.3}"), "-t", &format!("{dur:.3}")]);
                cmd.input(a_path.as_str());
                let ai = input_idx;
                input_idx += 1;
                cmd.args(["-ss", &format!("{b_src:.3}"), "-t", &format!("{dur:.3}")]);
                cmd.input(b_path.as_str());
                let bi = input_idx;
                input_idx += 1;

                filters.push_str(&fit(ai, "xa"));
                filters.push_str(&fit(bi, "xb"));
                let kind = if *dip { "fadeblack" } else { "fade" };
                filters.push_str(&format!(
                    "[xa{k}][xb{k}]xfade=transition={kind}:duration={dur:.3}:offset=0[v{k}];"
                ));

                // audio: crossfade both windows' audio, silence where absent
                let a_audio = if has_audio(a_path) {
                    filters.push_str(&format!("[{ai}:a]aresample=48000[xaa{k}];"));
                    format!("[xaa{k}]")
                } else {
                    cmd.args(["-f", "lavfi", "-t", &format!("{dur:.3}")]);
                    cmd.input("anullsrc=r=48000:cl=stereo");
                    input_idx += 1;
                    format!("[{}:a]", input_idx - 1)
                };
                let b_audio = if has_audio(b_path) {
                    filters.push_str(&format!("[{bi}:a]aresample=48000[xba{k}];"));
                    format!("[xba{k}]")
                } else {
                    cmd.args(["-f", "lavfi", "-t", &format!("{dur:.3}")]);
                    cmd.input("anullsrc=r=48000:cl=stereo");
                    input_idx += 1;
                    format!("[{}:a]", input_idx - 1)
                };
                filters.push_str(&format!("{a_audio}{b_audio}acrossfade=d={dur:.3}[a{k}];"));
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
        // audio beds carry no picture; only higher video tracks composite
        if !ov.audio_only {
            filters.push_str(&format!(
                "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},format=yuv420p,\
                 eq=brightness={b}:contrast={c}:saturation={sat},\
                 setpts=PTS-STARTPTS+{t0:.3}/TB[ov{j}];\
                 [{base}][ov{j}]overlay=eof_action=pass:enable='between(t,{t0:.3},{t1:.3})'[ovd{j}];",
                b = ov.fx.brightness,
                c = ov.fx.contrast,
                sat = ov.fx.saturation
            ));
            base = format!("ovd{j}");
        }
        if has_audio(&ov.path) {
            let ms = (ov.start * 1000.0).round() as i64;
            let vol = if ov.fx.volume != 1.0 {
                format!(",volume={:.4}", ov.fx.volume)
            } else {
                String::new()
            };
            filters.push_str(&format!(
                "[{vi}:a]aresample=48000{vol},adelay={ms}|{ms}[oa{j}];"
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
    // titles: chain drawtext over the program during each title's window
    if titles.is_empty() {
        filters.push_str(&format!("[{base}]null[outv]"));
    } else {
        let font = ff_path(&title_font());
        let mut cur = base.clone();
        for (ti, title) in titles.iter().enumerate() {
            let fs = ((title.font_size * h as f64 / 1080.0).round() as i64).max(8);
            let x = if title.pos_x.abs() < 1e-9 {
                "(w-text_w)/2".to_string()
            } else {
                format!("(w-text_w)/2+({:.0})", title.pos_x * w as f64)
            };
            let y = if title.pos_y.abs() < 1e-9 {
                "(h-text_h)/2".to_string()
            } else {
                format!("(h-text_h)/2+({:.0})", title.pos_y * h as f64)
            };
            let boxpart = if title.bg > 0.0 {
                format!(":box=1:boxcolor=black@{:.2}:boxborderw=14", title.bg.min(1.0))
            } else {
                String::new()
            };
            let (t0, t1) = (title.start, title.start + title.len);
            let out_lbl = format!("dt{ti}");
            filters.push_str(&format!(
                "[{cur}]drawtext=fontfile='{font}':text='{txt}':fontsize={fs}:\
                 fontcolor=white:x={x}:y={y}{boxpart}:\
                 enable='between(t,{t0:.3},{t1:.3})'[{out_lbl}];",
                txt = esc_drawtext(&title.text)
            ));
            cur = out_lbl;
        }
        filters.push_str(&format!("[{cur}]null[outv]"));
    }

    let out_args = output_args(s.format, s.quality, encoder);
    let mut child = cmd
        .args(["-filter_complex", &filters])
        .args(["-map", "[outv]", "-map", "[outa]"])
        .args(out_args.iter())
        .args(["-y"])
        .output(out.to_string_lossy())
        .spawn()?;

    let mut saw_error = None;
    for event in child.iter()? {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = child.kill();
            let _ = std::fs::remove_file(out); // drop the partial output
            anyhow::bail!("export cancelled");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: f64, len: f64, src_in: f64, trans_dur: f64) -> ExportClip {
        ExportClip {
            start,
            len,
            src_in,
            path: "x.mp4".into(),
            fx: ClipFx::default(),
            trans_dur,
            trans_dip: false,
            kf: BTreeMap::new(),
        }
    }

    #[test]
    fn keyframed_clip_samples_into_subsegments() {
        let mut c = clip(0.0, 4.0, 0.0, 0.0);
        c.kf.insert("scale".into(), vec![(0.0, 1.0), (4.0, 2.0)]);
        let segs = build_segments(vec![c]);
        // 4s / 0.2 = 20 sub-segments
        assert_eq!(segs.len(), 20);
        let total: f64 = segs.iter().map(|s| s.len()).sum();
        assert!((total - 4.0).abs() < 1e-6, "duration preserved, got {total}");
        // scale should rise across the sub-segments (animated)
        let first = match &segs[0] {
            Segment::Clip { fx, .. } => fx.scale,
            _ => panic!(),
        };
        let last = match segs.last().unwrap() {
            Segment::Clip { fx, .. } => fx.scale,
            _ => panic!(),
        };
        assert!(last > first + 0.5, "scale animated {first} -> {last}");
    }

    #[test]
    fn transition_shortens_prev_and_preserves_duration() {
        // A [0,4], B [4,4] with a 1s dissolve into B
        let segs = build_segments(vec![clip(0.0, 4.0, 2.0, 0.0), clip(4.0, 4.0, 3.0, 1.0)]);
        assert_eq!(segs.len(), 3);
        match &segs[0] {
            Segment::Clip { len, .. } => assert!((len - 3.0).abs() < 1e-9, "A body should be 3s"),
            _ => panic!("seg0 not clip"),
        }
        match &segs[1] {
            Segment::Transition { dur, a_src, b_src, .. } => {
                assert!((dur - 1.0).abs() < 1e-9);
                assert!((a_src - 5.0).abs() < 1e-9, "A tail window at src 2+4-1=5");
                assert!((b_src - 2.0).abs() < 1e-9, "B head window at src 3-1=2");
            }
            _ => panic!("seg1 not transition"),
        }
        let total: f64 = segs.iter().map(|s| s.len()).sum();
        assert!((total - 8.0).abs() < 1e-9, "total preserved at 8s, got {total}");
    }

    #[test]
    fn atempo_chains_beyond_2x() {
        assert_eq!(atempo_chain(1.5), "atempo=1.5000");
        assert_eq!(atempo_chain(4.0), "atempo=2.0,atempo=2.0000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5000");
    }

    #[test]
    fn transition_needs_adjacency() {
        // gap between clips → no transition even if requested
        let segs = build_segments(vec![clip(0.0, 4.0, 0.0, 0.0), clip(6.0, 4.0, 3.0, 1.0)]);
        assert!(segs.iter().all(|s| !matches!(s, Segment::Transition { .. })));
        assert!(segs.iter().any(|s| matches!(s, Segment::Gap { .. })));
    }
}

/// Build the segment list for a track from (start, len, src_in, path)
/// tuples: sorts, inserts gaps, ignores overlaps (later start wins the
/// overlap region is v2 territory — v1 assumes non-overlapping V1).
/// Build the V1 segment list from ordered clips, inserting gaps and
/// transitions. A transition into a clip shortens the previous clip's
/// tail by `dur` and inserts a Transition segment there, so the total
/// timeline duration is preserved (handle-based; see Segment::Transition).
pub fn build_segments(mut clips: Vec<ExportClip>) -> Vec<Segment> {
    clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    let mut segs: Vec<Segment> = Vec::new();
    let mut t = 0.0f64;
    let mut prev: Option<ExportClip> = None;
    let mut prev_keyframed = false;
    for c in clips {
        if c.start > t + 1e-3 {
            segs.push(Segment::Gap { len: c.start - t });
            prev = None; // a gap breaks adjacency
        }
        let adj = prev
            .as_ref()
            .map(|p| (p.start + p.len - c.start).abs() < 1e-3)
            .unwrap_or(false);
        // a transition shortens the previous clip's tail; skip if that
        // clip was keyframe-expanded (its tail is many small segments)
        let can_trans = c.trans_dur > 0.05
            && adj
            && !prev_keyframed
            && matches!(segs.last(), Some(Segment::Clip { .. }));
        if can_trans {
            let p = prev.as_ref().unwrap();
            let d = c.trans_dur.min(p.len - 0.05).min(c.len);
            if d > 0.05 {
                // shorten the previous clip body by d
                if let Some(Segment::Clip { len, .. }) = segs.last_mut() {
                    *len -= d;
                }
                segs.push(Segment::Transition {
                    a_path: p.path.clone(),
                    a_src: p.src_in + p.len - d,
                    b_path: c.path.clone(),
                    b_src: (c.src_in - d).max(0.0),
                    dur: d,
                    dip: c.trans_dip,
                });
            }
        }
        if c.kf.is_empty() {
            segs.push(Segment::Clip {
                path: c.path.clone(),
                src_in: c.src_in,
                len: c.len,
                fx: c.fx.clone(),
            });
        } else {
            // sample the animation into piecewise-constant sub-segments
            let n = ((c.len / KF_STEP_S).ceil() as usize).clamp(1, KF_MAX_SEGS);
            let step = c.len / n as f64;
            for i in 0..n {
                let rel = i as f64 * step;
                let sub_len = if i == n - 1 { c.len - rel } else { step };
                let fx = c.fx.sampled_at(&c.kf, rel + sub_len / 2.0);
                segs.push(Segment::Clip {
                    path: c.path.clone(),
                    src_in: c.src_in + rel * c.fx.speed, // source advances at speed
                    len: sub_len,
                    fx,
                });
            }
        }
        prev_keyframed = !c.kf.is_empty();
        t = c.start + c.len;
        prev = Some(c);
    }
    segs
}
