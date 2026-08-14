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

/// One censor region: blur / pixelate / black-box a rectangle. style is
/// 0 off / 1 blur / 2 pixelate / 3 solid; x/y = normalised centre, w/h =
/// normalised size, strength = 0..1. Up to 3 per clip, each keyframeable.
#[derive(Debug, Clone)]
pub struct CensorSlot {
    pub style: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub strength: f64,
    pub color: f64, // packed 0xRRGGBB for the solid box (0 = black)
}

impl Default for CensorSlot {
    fn default() -> Self {
        Self { style: 0.0, x: 0.5, y: 0.5, w: 0.25, h: 0.25, strength: 0.4, color: 0.0 }
    }
}

/// Map a flat fx key ("censor", "censor2_x", "censor3_str", …) to the slot
/// index (0..3) and which field it addresses.
fn parse_censor(key: &str) -> Option<(usize, char)> {
    let rest = key.strip_prefix("censor")?;
    let (idx, tail) = match rest.strip_prefix('2') {
        Some(t) => (1, t),
        None => match rest.strip_prefix('3') {
            Some(t) => (2, t),
            None => (0, rest),
        },
    };
    let field = match tail {
        "" => 's',
        "_x" => 'x',
        "_y" => 'y',
        "_w" => 'w',
        "_h" => 'h',
        "_str" => 'r',
        "_color" => 'c',
        _ => return None,
    };
    Some((idx, field))
}

/// Per-clip Effect Controls resolved to identity-defaulted numbers. The
/// same vocabulary the preview (CSS) and playback (gain) speak.
#[derive(Debug, Clone)]
pub struct ClipFx {
    pub brightness: f64,  // 0, range -1..1
    pub contrast: f64,    // 1
    pub saturation: f64,  // 1
    pub temperature: f64, // 0, -100 (cool) .. 100 (warm)
    pub tint: f64,        // 0, -100 (green) .. 100 (magenta)
    pub hue: f64,         // 0, degrees -180..180
    pub blur: f64,        // 0, gaussian sigma
    pub sharpen: f64,     // 0, 0..1 amount
    pub grain: f64,       // 0, 0..1 film-grain strength
    pub vignette: f64,    // 0, 0..1 strength
    pub flip_h: f64,      // 0 or 1
    pub flip_v: f64,      // 0 or 1
    pub chroma: f64,      // 0 or 1 — green-screen key (overlay tracks)
    pub chroma_sim: f64,  // 0.30 — key similarity
    pub denoise: f64,     // 0 or 1 — audio voice cleanup
    pub scale: f64,      // 1
    pub rot: f64,        // degrees
    pub pos_x: f64,      // fraction of width
    pub pos_y: f64,      // fraction of height
    pub fade_in: f64,    // seconds
    pub fade_out: f64,   // seconds
    pub volume: f64,     // 1
    pub speed: f64,      // 1 (2 = 2× faster, 0.5 = slow-mo)
    // up to 3 censor regions. Each field is keyframeable (censor_x,
    // censor2_x, censor3_x …) so a box can follow a moving subject.
    pub censor: [CensorSlot; 3],
}

impl Default for ClipFx {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            tint: 0.0,
            hue: 0.0,
            blur: 0.0,
            sharpen: 0.0,
            grain: 0.0,
            vignette: 0.0,
            flip_h: 0.0,
            flip_v: 0.0,
            chroma: 0.0,
            chroma_sim: 0.30,
            denoise: 0.0,
            scale: 1.0,
            rot: 0.0,
            pos_x: 0.0,
            pos_y: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            volume: 1.0,
            speed: 1.0,
            censor: [CensorSlot::default(), CensorSlot::default(), CensorSlot::default()],
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
            "temperature" => self.temperature = v,
            "tint" => self.tint = v,
            "hue" => self.hue = v,
            "blur" => self.blur = v,
            "sharpen" => self.sharpen = v,
            "grain" => self.grain = v,
            "vignette" => self.vignette = v,
            "flip_h" => self.flip_h = v,
            "flip_v" => self.flip_v = v,
            "chroma" => self.chroma = v,
            "chroma_sim" => self.chroma_sim = v,
            "denoise" => self.denoise = v,
            "scale" => self.scale = v,
            "rot" => self.rot = v,
            "pos_x" => self.pos_x = v,
            "pos_y" => self.pos_y = v,
            "speed" => self.speed = v,
            "fade_in" => self.fade_in = v,
            "fade_out" => self.fade_out = v,
            "volume" => self.volume = v,
            _ => {
                if let Some((i, f)) = parse_censor(key) {
                    let s = &mut self.censor[i];
                    match f {
                        's' => s.style = v,
                        'x' => s.x = v,
                        'y' => s.y = v,
                        'w' => s.w = v,
                        'h' => s.h = v,
                        'r' => s.strength = v,
                        'c' => s.color = v,
                        _ => {}
                    }
                }
            }
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
            temperature: g("temperature", d.temperature),
            tint: g("tint", d.tint),
            hue: g("hue", d.hue),
            blur: g("blur", d.blur),
            sharpen: g("sharpen", d.sharpen),
            grain: g("grain", d.grain),
            vignette: g("vignette", d.vignette),
            flip_h: g("flip_h", d.flip_h),
            flip_v: g("flip_v", d.flip_v),
            chroma: g("chroma", d.chroma),
            chroma_sim: g("chroma_sim", d.chroma_sim),
            denoise: g("denoise", d.denoise),
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
            censor: {
                let slot = |p: &str| CensorSlot {
                    style: g(p, 0.0),
                    x: g(&format!("{p}_x"), 0.5),
                    y: g(&format!("{p}_y"), 0.5),
                    w: g(&format!("{p}_w"), 0.25),
                    h: g(&format!("{p}_h"), 0.25),
                    strength: g(&format!("{p}_str"), 0.4),
                    color: g(&format!("{p}_color"), 0.0),
                };
                [slot("censor"), slot("censor2"), slot("censor3")]
            },
        }
    }
    fn has_transform(&self) -> bool {
        self.scale != 1.0 || self.rot != 0.0 || self.pos_x != 0.0 || self.pos_y != 0.0
    }
}

#[derive(Debug, Clone)]
pub enum Segment {
    Clip { path: String, src_in: f64, len: f64, fx: ClipFx, lut: String },
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
    pub lut: String, // .cube LUT path, empty = none
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
/// One censor region's filter chain: split the frame, crop the region,
/// blur / pixelate / black it, and overlay it back exactly in place. Returns
/// the appended filter string and the new `cur` label, or None when off.
/// LGPL-safe filters only (crop, gblur, scale-mosaic, drawbox, overlay).
fn censor_chain(k: usize, slot: usize, c: &CensorSlot, w: u32, h: u32, cur: &str) -> Option<(String, String)> {
    let cen = c.style.round() as i32;
    if !(1..=3).contains(&cen) || c.w <= 0.01 || c.h <= 0.01 {
        return None;
    }
    let iw = w as i64;
    let ih = h as i64;
    let cw = ((c.w.clamp(0.02, 1.0) * w as f64) as i64 & !1).clamp(2, iw);
    let ch = ((c.h.clamp(0.02, 1.0) * h as f64) as i64 & !1).clamp(2, ih);
    let cx = (((c.x.clamp(0.0, 1.0) * w as f64 - cw as f64 / 2.0) as i64).clamp(0, iw - cw)) & !1;
    let cy = (((c.y.clamp(0.0, 1.0) * h as f64 - ch as f64 / 2.0) as i64).clamp(0, ih - ch)) & !1;
    let st = c.strength.clamp(0.0, 1.0);
    let out = format!("cs{k}_{slot}");
    let f = match cen {
        3 => {
            let rgb = (c.color.round() as i64).clamp(0, 0xFF_FFFF);
            format!("[{cur}]drawbox=x={cx}:y={cy}:w={cw}:h={ch}:color=0x{rgb:06X}:t=fill[{out}];")
        }
        2 => {
            let block = (4.0 + st * 36.0) as i64;
            let dw = (cw / block).max(1);
            let dh = (ch / block).max(1);
            format!(
                "[{cur}]split=2[cb{k}_{slot}][cc{k}_{slot}];\
                 [cc{k}_{slot}]crop={cw}:{ch}:{cx}:{cy},scale={dw}:{dh}:flags=neighbor,scale={cw}:{ch}:flags=neighbor[cp{k}_{slot}];\
                 [cb{k}_{slot}][cp{k}_{slot}]overlay={cx}:{cy}[{out}];"
            )
        }
        _ => {
            let sigma = 6.0 + st * 54.0;
            format!(
                "[{cur}]split=2[cb{k}_{slot}][cc{k}_{slot}];\
                 [cc{k}_{slot}]crop={cw}:{ch}:{cx}:{cy},gblur=sigma={sigma:.1}[cp{k}_{slot}];\
                 [cb{k}_{slot}][cp{k}_{slot}]overlay={cx}:{cy}[{out}];"
            )
        }
    };
    Some((f, out))
}

fn clip_video_chain(
    vi: u32,
    k: usize,
    len: f64,
    w: u32,
    h: u32,
    fps: u32,
    fx: &ClipFx,
    lut: &str,
    reframe: Reframe,
    rx: f64,
    ry: f64,
) -> String {
    // retime: source span was len*speed; /speed compresses it to len
    let setpts = if (fx.speed - 1.0).abs() < 1e-9 {
        "setpts=PTS-STARTPTS".to_string()
    } else {
        format!("setpts=(PTS-STARTPTS)/{:.5}", fx.speed)
    };
    // temperature/tint as a midtone colour-balance shift (warm = +red/-blue)
    let cb = if fx.temperature != 0.0 || fx.tint != 0.0 {
        format!(
            ",colorbalance=rm={rm:.4}:gm={gm:.4}:bm={bm:.4}",
            rm = fx.temperature / 200.0,
            gm = -fx.tint / 200.0,
            bm = -fx.temperature / 200.0
        )
    } else {
        String::new()
    };
    // Brightness/contrast on luma. NB: `eq` would be the obvious filter but
    // it is GPL-only and absent from the LGPL ffmpeg we ship (shipping a GPL
    // build would impose GPL terms on Cutlass). lutyuv is LGPL and does the
    // same job: pivot contrast around mid-grey, then offset by brightness.
    let bc = if (fx.brightness).abs() > 1e-9 || (fx.contrast - 1.0).abs() > 1e-9 {
        format!(
            ",lutyuv=y='clip((val-(maxval+1)/2)*{c:.4}+(maxval+1)/2+({b:.4})*maxval,minval,maxval)'",
            c = fx.contrast,
            b = fx.brightness
        )
    } else {
        String::new()
    };
    // out_range=tv pins the frames to limited (broadcast) range. Without it
    // a full-range source reaches the encoder untagged and some encoders —
    // h264_amf notably — silently convert full→limited, washing the image
    // out (measured SSIM 0.977 vs 0.999) regardless of bitrate.
    // reframe: how the source fills a differently-shaped output (e.g. a 16:9
    // clip in a 9:16 Short). LGPL-safe filters only (scale/crop/gblur/overlay).
    let rx = rx.clamp(0.0, 1.0);
    let ry = ry.clamp(0.0, 1.0);
    let mut s = match reframe {
        Reframe::Letterbox => format!(
            "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease:out_range=tv,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},{setpts},\
             format=yuv420p{bc}{cb}[cf{k}];",
        ),
        Reframe::Fill => format!(
            "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=increase:out_range=tv,\
             crop={w}:{h}:(iw-{w})*{rx:.4}:(ih-{h})*{ry:.4},fps={fps},{setpts},\
             format=yuv420p{bc}{cb}[cf{k}];",
        ),
        Reframe::Blur => format!(
            "[{vi}:v]split=2[rb{k}][rf{k}];\
             [rb{k}]scale={w}:{h}:force_original_aspect_ratio=increase:out_range=tv,\
             crop={w}:{h},gblur=sigma=24,colorchannelmixer=rr=0.72:gg=0.72:bb=0.72[rbg{k}];\
             [rf{k}]scale={w}:{h}:force_original_aspect_ratio=decrease:out_range=tv[rff{k}];\
             [rbg{k}][rff{k}]overlay=(W-w)/2:(H-h)/2,fps={fps},{setpts},\
             format=yuv420p{bc}{cb}[cf{k}];",
        ),
    };
    let mut cur = format!("cf{k}");
    // stylize: LUT, hue/saturation, gaussian blur, mirror flips
    let mut stylize = String::new();
    if !lut.is_empty() {
        // same path escaping drawtext uses for fontfile
        stylize.push_str(&format!("lut3d=file='{}',", ff_path(lut)));
    }
    // saturation rides the (LGPL) hue filter's `s` param — eq's job again
    if fx.hue != 0.0 || (fx.saturation - 1.0).abs() > 1e-9 {
        stylize.push_str(&format!(
            "hue=h={:.2}:s={:.4},",
            fx.hue,
            fx.saturation.max(0.0)
        ));
    }
    if fx.blur > 0.01 {
        stylize.push_str(&format!("gblur=sigma={:.2},", fx.blur));
    }
    if fx.sharpen > 0.01 {
        // luma unsharp; amount scales 0..~2.5
        stylize.push_str(&format!("unsharp=5:5:{:.3}:5:5:0,", fx.sharpen * 2.5));
    }
    if fx.grain > 0.01 {
        // temporal noise = animated film grain
        stylize.push_str(&format!("noise=alls={}:allf=t,", (fx.grain * 22.0).round() as i64));
    }
    if fx.flip_h > 0.5 {
        stylize.push_str("hflip,");
    }
    if fx.flip_v > 0.5 {
        stylize.push_str("vflip,");
    }
    if !stylize.is_empty() {
        let f = stylize.trim_end_matches(',');
        s.push_str(&format!("[{cur}]{f}[st{k}];"));
        cur = format!("st{k}");
    }
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
    if fx.vignette > 0.01 {
        // ffmpeg's `angle` IS the strength: 0 = none, larger = darker corners
        // (its default PI/5 ≈ 0.63 is already pronounced). This used to be
        // inverted — 0.2 strength produced 0.78, darker than the default,
        // while 1.0 produced 0.30 — so exports came out far darker than the
        // preview's 20%-opacity overlay.
        let ang = fx.vignette.clamp(0.0, 1.0) * 0.9;
        s.push_str(&format!("[{cur}]vignette=angle={ang:.3}[vg{k}];"));
        cur = format!("vg{k}");
    }
    // censor: up to 3 rectangular regions, applied in order over the frame.
    for (slot, c) in fx.censor.iter().enumerate() {
        if let Some((f, out)) = censor_chain(k, slot, c, w, h, &cur) {
            s.push_str(&f);
            cur = out;
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
    if fx.denoise > 0.5 {
        // FFT denoise cleans up voice hiss / room tone
        c.push_str(",afftdn=nr=12:nf=-25");
    }
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
    /// NB: x264/x265 are GPL and absent from the LGPL ffmpeg we ship, so
    /// H.264 falls back to libopenh264 (LGPL software) and H.265 relies on
    /// hardware encoders.
    fn encoders(self) -> &'static [&'static str] {
        match self {
            Self::Mp4H264 => &["h264_qsv", "h264_nvenc", "h264_amf", "libopenh264"],
            // hevc_amf was missing, so H.265 failed outright on AMD machines
            Self::Mp4H265 => &["hevc_qsv", "hevc_nvenc", "hevc_amf", "hevc_mf"],
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
    /// how a source that doesn't match the output aspect fills the frame
    pub reframe: Reframe,
    /// pan for Fill mode: 0..1 in each axis (0.5 = centred)
    pub reframe_x: f64,
    pub reframe_y: f64,
}

/// How a clip fills the output frame when its aspect differs (e.g. a 16:9
/// source in a 9:16 Short).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reframe {
    /// fit inside with black bars (the classic letterbox)
    Letterbox,
    /// zoom to cover the frame, cropping the overflow (native vertical look)
    Fill,
    /// fit centred over a blurred, zoomed copy of itself (no crop, no bars)
    Blur,
}

impl Default for Reframe {
    fn default() -> Self {
        Reframe::Letterbox
    }
}

impl Reframe {
    pub fn parse(s: &str) -> Self {
        match s {
            "fill" => Reframe::Fill,
            "blur" => Reframe::Blur,
            _ => Reframe::Letterbox,
        }
    }
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            format: ExportFormat::Mp4H264,
            quality: Quality::Medium,
            reframe: Reframe::Letterbox,
            reframe_x: 0.5,
            reframe_y: 0.5,
        }
    }
}

/// The output-encoding args for a format/quality/encoder combination —
/// everything after the filtergraph maps.
/// Bits/sec target, scaled from a 1080p30 base by frame area and frame
/// rate. We drive every H.264/H.265 encoder by explicit bitrate: hardware
/// encoders honour quality factors inconsistently (h264_qsv given
/// `-global_quality 20` actually encodes at q≈23 and measured *worse* SSIM
/// than a plain bitrate target), so a generous VBR target is both better
/// looking and predictable across QSV/NVENC/AMF/openh264.
fn target_bitrate_bps(width: u32, height: u32, fps: u32, quality: Quality, hevc: bool) -> u64 {
    // Generous targets on purpose: with x264/x265 unavailable (GPL) we lean
    // on hardware encoders, which need noticeably more bits than x264 for
    // the same look. Trading file size for quality is the right default for
    // an editor's master export.
    let base: f64 = match quality {
        Quality::Low => 8_000_000.0,
        Quality::Medium => 16_000_000.0,
        Quality::High => 40_000_000.0,
    };
    let area = ((width as f64 * height as f64) / (1920.0 * 1080.0)).clamp(0.25, 4.0);
    let rate = (fps.max(1) as f64 / 30.0).clamp(0.5, 2.0);
    // HEVC is roughly twice as efficient for the same perceived quality
    let codec = if hevc { 0.6 } else { 1.0 };
    (base * area * rate * codec).round() as u64
}

/// Tag the output limited-range bt709 (standard HD delivery). The filter
/// chain already pins the pixels to tv range; tagging it means players and
/// encoders agree instead of guessing.
fn color_tag_args() -> Vec<String> {
    [
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ]
    .iter()
    .map(|x| x.to_string())
    .collect()
}

/// `-b:v/-maxrate/-bufsize` VBR triple for the computed target. With
/// `cbr`, pin min/max to the target too — a last resort for encoders that
/// accept the bitrate and then emit a fraction of it.
fn rate_args(
    width: u32,
    height: u32,
    fps: u32,
    quality: Quality,
    hevc: bool,
    cbr: bool,
) -> Vec<String> {
    let b = target_bitrate_bps(width, height, fps, quality, hevc);
    if cbr {
        return vec![
            "-b:v".into(),
            b.to_string(),
            "-minrate".into(),
            b.to_string(),
            "-maxrate".into(),
            b.to_string(),
            "-bufsize".into(),
            b.to_string(),
        ];
    }
    vec![
        "-b:v".into(),
        b.to_string(),
        "-maxrate".into(),
        (b * 3 / 2).to_string(),
        "-bufsize".into(),
        (b * 2).to_string(),
    ]
}

fn output_args(
    format: ExportFormat,
    quality: Quality,
    encoder: &str,
    width: u32,
    height: u32,
    fps: u32,
    cbr: bool,
) -> Vec<String> {
    let s = |x: &str| x.to_string();
    // AMD's AMF encoders honour -b:v only in CBR; their VBR modes emit a
    // fraction of the target. Use CBR from the first pass so we don't encode
    // once in VBR, reject it, and re-encode in CBR — that double pass doubled
    // export time and made the progress bar fill then reset.
    let cbr = cbr || encoder.ends_with("_amf");
    let mut a = vec![s("-c:v"), s(encoder)];
    match format {
        ExportFormat::Mp4H264 => {
            a.extend(rate_args(width, height, fps, quality, false, cbr));
            match encoder {
                "h264_nvenc" => a.extend([s("-rc"), if cbr { s("cbr") } else { s("vbr") }]),
                // AMF defaults to a speed preset and Main profile; ask for
                // quality + High so it uses the better compression tools.
                "h264_amf" => {
                    a.extend([s("-quality"), s("2"), s("-profile:v"), s("high")]);
                    if cbr {
                        a.extend([s("-rc"), s("cbr")]);
                    }
                }
                _ => {}
            }
            a.extend([s("-pix_fmt"), s("yuv420p")]);
            a.extend(color_tag_args());
            a.extend([s("-c:a"), s("aac"), s("-b:a"), s("192k")]);
            a.extend([s("-movflags"), s("+faststart")]);
        }
        ExportFormat::Mp4H265 => {
            a.extend(rate_args(width, height, fps, quality, true, cbr));
            match encoder {
                "hevc_nvenc" => a.extend([s("-rc"), if cbr { s("cbr") } else { s("vbr") }]),
                "hevc_amf" => {
                    a.extend([s("-quality"), s("2")]);
                    if cbr {
                        a.extend([s("-rc"), s("cbr")]);
                    }
                }
                _ => {}
            }
            a.extend([s("-pix_fmt"), s("yuv420p")]);
            a.extend([s("-tag:v"), s("hvc1")]); // QuickTime-playable HEVC
            a.extend(color_tag_args());
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
        let mut res = run_export(
            segments, overlays, titles, out, settings, encoder, total, progress, cancel, false,
        );
        // If it accepted -b:v but emitted a fraction of it, retry the same
        // encoder pinned to CBR, which obliges it to hit the rate. Hardware
        // encoders honour -b:v inconsistently depending on driver/context,
        // and for H.265 there is no software fallback to fall back to.
        if !cancel.load(Ordering::Relaxed)
            && res.as_ref().err().is_some_and(|e| e.to_string().contains("ignored the requested"))
        {
            eprintln!("{encoder} under-delivered; retrying pinned to CBR");
            progress(0.0);
            res = run_export(
                segments, overlays, titles, out, settings, encoder, total, progress, cancel, true,
            );
        }
        match res {
            Ok(()) => return Ok(encoder.to_string()),
            Err(e) => {
                // a user cancel must not fall through to the next encoder
                if cancel.load(Ordering::Relaxed) {
                    return Err(e);
                }
                if i + 1 < encoders.len() {
                    eprintln!("{encoder} encode failed ({e:#}); trying {}", encoders[i + 1]);
                } else if matches!(settings.format, ExportFormat::Mp4H265) {
                    // Every HEVC encoder is hardware-backed here (x265 is GPL,
                    // so there's no software fallback). Plenty of GPUs encode
                    // H.264 but not HEVC — say so plainly instead of leaking
                    // a driver error like MF_E_INVALIDTYPE.
                    return Err(anyhow::anyhow!(
                        "H.265 isn't available on this machine — no working HEVC \
                         encoder was found (many GPUs support H.264 encoding but \
                         not HEVC). Export as MP4 H.264 (universal) instead, which \
                         plays everywhere. Details: {e:#}"
                    ));
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
    cbr: bool,
) -> anyhow::Result<()> {
    let (w, h, fps) = (s.width, s.height, s.fps);
    let mut cmd = FfmpegCommand::new();
    let mut filters = String::new();
    let mut concat_inputs = String::new();
    let mut input_idx = 0u32;

    for (k, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Clip { path, src_in, len, fx, lut } => {
                // input-level seek: ffmpeg decodes from the prior keyframe
                // and discards up to the exact point — frame-accurate.
                // With speed, consume `len*speed` of source (retimed to
                // `len` on the timeline by setpts/atempo in the chains).
                let src_dur = len * fx.speed;
                cmd.args(["-ss", &format!("{src_in:.3}"), "-t", &format!("{src_dur:.3}")]);
                cmd.input(path.as_str());
                let vi = input_idx;
                input_idx += 1;
                filters.push_str(&clip_video_chain(
                    vi, k, *len, w, h, fps, fx, lut,
                    s.reframe, s.reframe_x, s.reframe_y,
                ));
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
            // green-screen key: alpha format + colorkey so the base shows
            // through the keyed pixels
            let (pix, key) = if ov.fx.chroma > 0.5 {
                (
                    "yuva420p",
                    format!(
                        ",colorkey=0x00D000:similarity={:.3}:blend=0.10",
                        ov.fx.chroma_sim.clamp(0.01, 1.0)
                    ),
                )
            } else {
                ("yuv420p", String::new())
            };
            filters.push_str(&format!(
                "[{vi}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
                 pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,fps={fps},format={pix},\
                 eq=brightness={b}:contrast={c}:saturation={sat}{key},\
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

    let out_args = output_args(s.format, s.quality, encoder, s.width, s.height, s.fps, cbr);
    // CUTLASS_DEBUG_FFMPEG=1 dumps the graph + output args — the fastest way
    // to see what the export actually asked ffmpeg to do.
    if std::env::var("CUTLASS_DEBUG_FFMPEG").is_ok() {
        eprintln!("--- filter_complex ---\n{filters}\n--- out_args ---\n{out_args:?}\n");
    }
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

    // Some encoders accept -b:v and then silently emit a small fraction of
    // it (driver/Media-Foundation dependent), which looks terrible even
    // though ffmpeg reports success. Treat a wildly-under-target file as a
    // failure so export() falls through to the next encoder.
    if matches!(s.format, ExportFormat::Mp4H264 | ExportFormat::Mp4H265) {
        let hevc = matches!(s.format, ExportFormat::Mp4H265);
        let target = target_bitrate_bps(s.width, s.height, s.fps, s.quality, hevc);
        if let Ok(meta) = std::fs::metadata(out) {
            let actual = (meta.len() as f64 * 8.0 / total.max(0.1)) as u64;
            if actual * 5 < target * 2 {
                anyhow::bail!(
                    "{encoder} ignored the requested bitrate ({} kbps of {} kbps)",
                    actual / 1000,
                    target / 1000
                );
            }
        }
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
            lut: String::new(),
            trans_dur,
            trans_dip: false,
            kf: BTreeMap::new(),
        }
    }

    #[test]
    fn censor_emits_region_filters() {
        let (w, h, fps) = (1920u32, 1080u32, 30u32);
        let mut fx = ClipFx::default();
        fx.censor[0].style = 1.0; // blur
        let blur = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Letterbox, 0.5, 0.5);
        assert!(blur.contains("crop="), "blur crops the region");
        assert!(blur.contains("gblur=sigma"), "blur uses gblur");
        assert!(blur.contains("overlay="), "blur composites back");

        fx.censor[0].style = 2.0; // pixelate → neighbor scale mosaic
        let pix = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Letterbox, 0.5, 0.5);
        assert!(pix.contains("flags=neighbor"), "pixelate uses neighbor scale");

        fx.censor[0].style = 3.0; // solid box, custom colour
        fx.censor[0].color = 0xFF8800 as f64;
        let solid = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Letterbox, 0.5, 0.5);
        assert!(solid.contains("drawbox="), "solid fills a box");
        assert!(solid.contains("color=0xFF8800"), "solid honours the picked colour");

        // off → no censor filters
        let off = clip_video_chain(0, 0, 4.0, w, h, fps, &ClipFx::default(), "", Reframe::Letterbox, 0.5, 0.5);
        assert!(!off.contains("drawbox=") && !off.contains("crop="));
    }

    #[test]
    fn reframe_fill_and_blur_emit_filters() {
        let (w, h, fps) = (1080u32, 1920u32, 30u32); // vertical (9:16) target
        let fx = ClipFx::default();
        let letter = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Letterbox, 0.5, 0.5);
        assert!(letter.contains("pad=1080:1920"), "letterbox pads to bars");
        let fill = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Fill, 0.5, 0.5);
        assert!(
            fill.contains("force_original_aspect_ratio=increase") && fill.contains("crop=1080:1920"),
            "fill scales to cover then crops"
        );
        assert!(!fill.contains("pad="), "fill leaves no bars");
        let blur = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Blur, 0.5, 0.5);
        assert!(blur.contains("gblur=sigma") && blur.contains("overlay="), "blur bg + fit overlay");
    }

    #[test]
    fn multiple_censor_boxes_chain_independently() {
        let (w, h, fps) = (1920u32, 1080u32, 30u32);
        let mut fx = ClipFx::default();
        fx.censor[0].style = 1.0; // blur
        fx.censor[1].style = 3.0; // solid
        fx.censor[2].style = 2.0; // pixelate
        let chain = clip_video_chain(0, 0, 4.0, w, h, fps, &fx, "", Reframe::Letterbox, 0.5, 0.5);
        // three distinct region outputs, threaded in order
        assert!(chain.contains("[cs0_0]"), "slot 0 output present");
        assert!(chain.contains("[cs0_1]"), "slot 1 output present");
        assert!(chain.contains("[cs0_2]"), "slot 2 output present");
        assert!(chain.contains("drawbox="), "solid box (slot 1)");
        assert!(chain.contains("gblur=sigma"), "blur (slot 0)");
        assert!(chain.contains("flags=neighbor"), "pixelate (slot 2)");
        // from_map round-trips the numbered keys
        let m: BTreeMap<String, f64> =
            [("censor2".to_string(), 3.0), ("censor2_x".to_string(), 0.8)].into_iter().collect();
        let fx2 = ClipFx::from_map(&m);
        assert_eq!(fx2.censor[1].style, 3.0);
        assert!((fx2.censor[1].x - 0.8).abs() < 1e-9);
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
                lut: c.lut.clone(),
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
                    lut: c.lut.clone(),
                });
            }
        }
        prev_keyframed = !c.kf.is_empty();
        t = c.start + c.len;
        prev = Some(c);
    }
    segs
}
