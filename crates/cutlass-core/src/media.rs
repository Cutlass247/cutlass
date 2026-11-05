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

/// Frames in the quick first pass.
///
/// Each one costs a seek and a keyframe decode — about 35 ms at 1080p — so the
/// whole 240 take roughly a minute on a half-hour recording, and the clip used
/// to wait on all of them. Twenty-four is about a second: enough that the strip
/// reads as a filmstrip rather than an empty bar, while the full set is built
/// behind it.
pub const COARSE_SCRUB_FRAMES: f64 = 24.0;

/// Under this, the full pass is quick enough that splitting it in two would
/// cost more than it saves — a clip this short is done before anyone looks.
pub const COARSE_PASS_ABOVE_S: f64 = 20.0;

/// Pub because the app's engine-based import writes these files directly, and
/// a prefix that disagrees with the reader here means a strip that silently
/// stays empty.
pub const FULL_FRAME_PREFIX: char = 'f';
pub const COARSE_FRAME_PREFIX: char = 'c';

/// What an imported file is. Decides which track it can go on and what the
/// bin draws for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// Has a picture (and usually sound).
    Video,
    /// Sound only: no thumbnails, no frame rate, never on a video track.
    Audio,
    /// A still picture: no length of its own and no sound, so its clip can
    /// be any length, and export loops it rather than seeking in it.
    Image,
}

/// File types that are always a single picture: every picture type but GIF.
const STILL_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "bmp", "tif", "tiff", "heic", "heif", "hif",
    "avif", "jxl", "jp2", "j2k", "psd", "tga", "ico", "qoi", "dds", "pcx", "ppm", "pgm", "pbm",
    "pam", "sgi", "ras", "xbm", "xpm", "xwd",
];

/// Pictures stored the HEIF way: an iPhone's .heic, .heif, Canon's .hif,
/// AVIF. The picture can be a grid of tiles (an iPhone's is 35 of them, each
/// 512 square) and the turn it's shown at is a separate property; the shipped
/// ffmpeg reads both but applies neither, so a photo came out as one corner
/// tile, or whole but on its side. Each is made into a PNG once
/// ([`convert_picture`]) and everything else reads that.
const HEIF_EXTS: &[&str] = &["heic", "heif", "hif", "avif"];

/// Whether `path` is a picture that has to be made into a PNG before use:
/// the HEIF ones, and the two whose own readers don't loop, which is how
/// export holds a picture on screen (an icon, a one-frame GIF; export's
/// looping reader, image2, can open neither).
pub fn needs_conversion(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    HEIF_EXTS.contains(&ext.as_str()) || ext == "ico" || (ext == "gif" && is_still(path))
}

/// One step of what a HEIF picture says to do to its image before showing
/// it, in the order the file lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeifStep {
    /// 'irot': this many quarter turns anticlockwise (1..=3).
    TurnAnticlockwise(u8),
    /// 'imir': 0 mirrors left to right, 1 top to bottom.
    Mirror(u8),
}

/// The ISO boxes directly inside `data`: (type, contents).
fn iso_boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size32 = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let (header, size) = match size32 {
            0 => (8, data.len() - at),
            1 if at + 16 <= data.len() => {
                (16, u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap()) as usize)
            }
            n => (8, n),
        };
        if size < header || at + size > data.len() {
            break;
        }
        out.push((kind, &data[at + header..at + size]));
        at += size;
    }
    out
}

/// The turn and mirror a HEIF file asks for its primary picture, read from
/// its boxes: `pitm` names the picture, `ipma` lists the properties that
/// apply to it, in order, and `ipco` holds them. Nothing (an empty list) for
/// anything that isn't a HEIF file or doesn't say.
pub fn heif_transform(file: &[u8]) -> Vec<HeifStep> {
    let Some((_, meta)) = iso_boxes(file).into_iter().find(|(k, _)| k == b"meta") else {
        return Vec::new();
    };
    // meta is a full box: four bytes of version and flags before its boxes
    let Some(inner) = meta.get(4..) else { return Vec::new() };
    let children = iso_boxes(inner);
    let primary = children.iter().find(|(k, _)| k == b"pitm").and_then(|(_, b)| match b.first()? {
        0 => Some(u16::from_be_bytes(b.get(4..6)?.try_into().ok()?) as u32),
        _ => Some(u32::from_be_bytes(b.get(4..8)?.try_into().ok()?)),
    });
    let Some(primary) = primary else { return Vec::new() };
    let Some((_, iprp)) = children.iter().find(|(k, _)| k == b"iprp") else { return Vec::new() };
    let props = iso_boxes(iprp);
    let ipco: Vec<([u8; 4], &[u8])> = props
        .iter()
        .find(|(k, _)| k == b"ipco")
        .map(|(_, b)| iso_boxes(b))
        .unwrap_or_default();
    let Some((_, ipma)) = props.iter().find(|(k, _)| k == b"ipma") else { return Vec::new() };

    // ipma: version and flags, an entry count, then per item its id (16 bits
    // in version 0, else 32), a count, and that many property indices (7 bits,
    // or 15 when flags bit 0 is set; the top bit marks "essential"), 1-based
    // into ipco.
    let (version, wide) = match (ipma.first(), ipma.get(3)) {
        (Some(v), Some(f)) => (*v, f & 1 == 1),
        _ => return Vec::new(),
    };
    let read = |at: &mut usize, n: usize| -> Option<u32> {
        let v = ipma.get(*at..*at + n)?.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32);
        *at += n;
        Some(v)
    };
    let mut at = 4usize;
    let Some(entries) = read(&mut at, 4) else { return Vec::new() };
    for _ in 0..entries {
        let Some(item) = read(&mut at, if version < 1 { 2 } else { 4 }) else { break };
        let Some(count) = read(&mut at, 1) else { break };
        let mut indices = Vec::new();
        for _ in 0..count {
            let Some(raw) = read(&mut at, if wide { 2 } else { 1 }) else { break };
            indices.push(raw & if wide { 0x7fff } else { 0x7f });
        }
        if item != primary {
            continue;
        }
        return indices
            .into_iter()
            .filter_map(|i| ipco.get((i as usize).checked_sub(1)?))
            .filter_map(|(kind, body)| match kind {
                b"irot" => Some(HeifStep::TurnAnticlockwise(body.first()? & 0x03)),
                b"imir" => Some(HeifStep::Mirror(body.first()? & 0x01)),
                _ => None,
            })
            .filter(|s| *s != HeifStep::TurnAnticlockwise(0))
            .collect();
    }
    Vec::new()
}

/// Where each tile of a gridded picture goes: (stream index, x, y) for each
/// tile, then the crop of the assembled grid that is the picture (x, y,
/// width, height). Read by the engine, which links libav; see
/// `cutlass_engine::tile_grid`.
#[derive(Debug, Clone)]
pub struct TileLayout {
    pub tiles: Vec<(usize, i32, i32)>,
    pub crop: (i32, i32, i32, i32),
}

/// A PNG's width and height, from its header.
pub fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    (head[..8] == *b"\x89PNG\r\n\x1a\n" && head[12..16] == *b"IHDR").then(|| {
        (
            u32::from_be_bytes(head[16..20].try_into().unwrap()),
            u32::from_be_bytes(head[20..24].try_into().unwrap()),
        )
    })
}

/// What [`convert_picture`] names its copy.
pub const CONVERTED_NAME: &str = "picture.png";

/// Make a picture that needs it (see [`needs_conversion`]) into a PNG, once:
/// a HEIF one's tiles assembled by `layout` and cropped, then turned and
/// mirrored as the file asks, at most 3840 on its long side (4K's, never
/// enlarged), transparency kept. A prior copy is reused.
pub fn convert_picture(src: &Path, layout: Option<&TileLayout>) -> anyhow::Result<PathBuf> {
    ensure_ffmpeg()?;
    let out = cache_dir(src)?.join(CONVERTED_NAME);
    if out.exists() {
        return Ok(out);
    }
    // Tiles are named stream by stream; a picture that's one image is left to
    // ffmpeg's own choice of stream, which is the largest: the full size of
    // a multi-size icon, the photo rather than its thumbnail.
    let (mut graph, picked) = match layout {
        Some(l) if l.tiles.len() > 1 => {
            let inputs: String = l.tiles.iter().map(|(i, ..)| format!("[0:{i}]")).collect();
            let places: Vec<String> = l.tiles.iter().map(|(_, x, y)| format!("{x}_{y}")).collect();
            let (cx, cy, cw, ch) = l.crop;
            let g = format!(
                "{inputs}xstack=inputs={}:layout={},crop={cw}:{ch}:{cx}:{cy}",
                l.tiles.len(),
                places.join("|")
            );
            (g, true)
        }
        Some(l) => {
            let (cx, cy, cw, ch) = l.crop;
            (format!("[0:{}]crop={cw}:{ch}:{cx}:{cy}", l.tiles.first().map_or(0, |t| t.0)), true)
        }
        None => ("null".to_string(), false),
    };
    for step in heif_transform(&std::fs::read(src)?) {
        graph.push_str(match step {
            HeifStep::TurnAnticlockwise(1) => ",transpose=cclock",
            HeifStep::TurnAnticlockwise(2) => ",hflip,vflip",
            HeifStep::TurnAnticlockwise(_) => ",transpose=clock",
            HeifStep::Mirror(0) => ",hflip",
            HeifStep::Mirror(_) => ",vflip",
        });
    }
    graph.push_str(",scale='min(3840,iw)':'min(3840,ih)':force_original_aspect_ratio=decrease");
    let tmp = out.with_extension("partial.png");
    // -noautorotate: the turn is applied above, from the file's own property,
    // and must not be applied twice should a later ffmpeg start reading it.
    let mut cmd = FfmpegCommand::new();
    cmd.args(["-noautorotate"]).input(src.to_string_lossy());
    if picked {
        cmd.args(["-filter_complex", &format!("{graph}[out]"), "-map", "[out]"]);
    } else {
        cmd.args(["-vf", &graph]);
    }
    let mut child = cmd
        .args(["-frames:v", "1", "-update", "1", "-pix_fmt", "rgba", "-y"])
        .output(tmp.to_string_lossy())
        .spawn()?;
    if let Ok(events) = child.iter() {
        for _ in events {}
    }
    anyhow::ensure!(
        child.wait().map(|s| s.success()).unwrap_or(false) && tmp.exists(),
        "couldn't read the picture in {}",
        src.display()
    );
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// Whether `path` is a still picture rather than footage, the one test both
/// import and export use. A GIF can be either: an animated one is a video.
/// Measured on the shipped ffmpeg, a one-frame GIF reads as 0.04s of video.
pub fn is_still(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if STILL_EXTS.contains(&ext.as_str()) {
        return true;
    }
    ext == "gif" && probe_duration_s(Path::new(path)).map_or(true, |d| d <= 0.05)
}

/// The picture the preview shows for a still: a PNG, so transparency survives
/// (a logo over footage), no bigger than 1920 on its long side and never
/// enlarged. Made by the shipped ffmpeg, which reads every type in
/// `IMAGE_EXTS`; the webview can't show TIFF at all. A prior one is reused.
pub fn still_preview(src: &Path) -> anyhow::Result<PathBuf> {
    ensure_ffmpeg()?;
    let out = cache_dir(src)?.join("still.png");
    if out.exists() {
        return Ok(out);
    }
    let tmp = out.with_extension("partial.png");
    let mut child = FfmpegCommand::new()
        .input(src.to_string_lossy())
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(1920,iw)':'min(1920,ih)':force_original_aspect_ratio=decrease",
            "-pix_fmt",
            "rgba",
            "-y",
        ])
        .output(tmp.to_string_lossy())
        .spawn()?;
    if let Ok(events) = child.iter() {
        for _ in events {}
    }
    anyhow::ensure!(
        child.wait().map(|s| s.success()).unwrap_or(false) && tmp.exists(),
        "couldn't read the picture in {}",
        src.display()
    );
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

/// File extensions Cutlass imports, by kind. The Import dialog and a drop on
/// the window offer exactly these (`apps/desktop/src/ipc.ts` keeps a copy,
/// held equal by a test), and `every_offered_file_type_imports` in the app
/// imports a real file of each one through the shipped ffmpeg.
///
/// The dialog used to offer six video types while a drop accepted seventeen,
/// so an MP3 could be dropped but never picked from Import.
///
/// Not offered: .oga. It almost always holds FLAC, and after a seek libav
/// times FLAC-in-Ogg about 0.8s wrong (frames labelled 2.0s carry the sound
/// from 1.2s), in playback and in export's `-ss` alike. Ogg Vorbis (.ogg) and
/// Opus seek correctly.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "asf", "flv", "f4v", "mpg", "mpeg",
    "vob", "ts", "mts", "m2ts", "3gp", "3g2", "ogv", "mxf", "dv",
];
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "wav", "m4a", "m4b", "aac", "flac", "ogg", "opus", "wma", "aif", "aiff",
    "ac3", "mka", "caf", "wv", "amr", "mp2",
];
/// Pictures: every still type the shipped ffmpeg decodes that looks right
/// without colour management. A GIF is imported as a picture or a video by
/// whether it moves; see [`is_still`].
///
/// Left out: OpenEXR, Radiance HDR and DPX store linear or film-log light,
/// which would come out dark or flat without tone mapping; SVG and camera
/// RAW, which this ffmpeg can't decode at all.
pub const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "bmp", "gif", "tif", "tiff", "heic", "heif",
    "hif", "avif", "jxl", "jp2", "j2k", "psd", "tga", "ico", "qoi", "dds", "pcx", "ppm", "pgm",
    "pbm", "pam", "sgi", "ras", "xbm", "xpm", "xwd",
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct MediaInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub kind: MediaKind,
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

/// Transcode `src` to a constant-frame-rate working copy in its cache dir and
/// return that copy's path. Variable-frame-rate sources (screen recordings
/// especially) place their frames at irregular timestamps; once such a clip is
/// cut and reassembled, timeline time no longer maps evenly onto frames and the
/// audio drifts out of sync — in the preview and the export alike. Editing the
/// conformed copy instead makes every downstream step (seek, preview, cut,
/// concat) frame-regular. A prior copy is reused. Hardware encoders are tried
/// first (they're fast and `-r` makes even QSV accept the stream), software
/// last so the conform always succeeds somewhere.
pub fn conform_to_cfr(src: &Path, fps: u32, bitrate: u64) -> anyhow::Result<PathBuf> {
    reencode(src, &format!("cfr{fps}"), Some(fps), None, bitrate)
}

/// How an HDR picture is brought into SDR (BT.709) for the working copy:
/// to linear light, the BT.2020 primaries to BT.709's, `mobius` to roll the
/// highlights off rather than clip them, and back to BT.709's curve.
/// Measured on a 4K HLG clip it runs at about 10 frames a second on the
/// CPU, so a minute of 4K30 takes three minutes, in the background.
pub const SDR_TONE_MAP: &str = "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,\
     tonemap=tonemap=mobius:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// Transcode an HDR source (HLG or PQ) to an SDR working copy, tone-mapped
/// by [`SDR_TONE_MAP`] and tagged BT.709, at a constant `fps` too when the
/// source is also variable-frame-rate. Without it HDR is shown and exported
/// washed out and grey, since nothing else here manages colour. A prior copy
/// is reused.
pub fn make_sdr_copy(src: &Path, fps: Option<u32>, bitrate: u64) -> anyhow::Result<PathBuf> {
    let name = fps.map_or("sdr".to_string(), |f| format!("sdr{f}"));
    reencode(src, &name, fps, Some(SDR_TONE_MAP), bitrate)
}

/// Re-encode `src` to `<name>.mp4` in its cache dir: at a constant `fps` if
/// given, through the filter `vf` if given (then tagged BT.709, which is what
/// the only filter passed here makes). Shared by [`conform_to_cfr`] and
/// [`make_sdr_copy`].
fn reencode(src: &Path, name: &str, fps: Option<u32>, vf: Option<&str>, bitrate: u64) -> anyhow::Result<PathBuf> {
    ensure_ffmpeg()?;
    let dir = cache_dir(src)?;
    let out = dir.join(format!("{name}.mp4"));
    if out.exists() {
        return Ok(out);
    }
    // encode to a temp name first so a crash never leaves a half-written file
    // that later looks like a valid cached conform.
    let tmp = dir.join(format!("{name}.partial.mp4"));
    let src_s = src.to_string_lossy().to_string();
    let tmp_s = tmp.to_string_lossy().to_string();
    let maxrate = (bitrate * 3 / 2).to_string();
    let bufsize = (bitrate * 2).to_string();
    let b = bitrate.to_string();
    for enc in ["h264_qsv", "h264_nvenc", "h264_amf", "libopenh264"] {
        let _ = std::fs::remove_file(&tmp);
        let mut cmd = FfmpegCommand::new();
        cmd.input(src_s.as_str());
        if let Some(fps) = fps {
            // -fps_mode cfr + explicit -r rewrites the irregular VFR cadence to
            // a clean constant rate; audio is left on its own clock so the two
            // align.
            cmd.args(["-fps_mode", "cfr", "-r", &fps.to_string()]);
        }
        if let Some(vf) = vf {
            cmd.args(["-vf", vf]);
            cmd.args(["-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709"]);
        }
        cmd.args(["-c:v", enc, "-b:v", b.as_str(), "-maxrate", maxrate.as_str(), "-bufsize", bufsize.as_str()]);
        if enc.ends_with("_amf") {
            // AMF only honours the bitrate in CBR; VBR emits a fraction of it.
            cmd.args(["-rc", "cbr", "-minrate", b.as_str(), "-quality", "2"]);
        }
        cmd.args([
            // 256k for the same reason the export uses it: ffmpeg's AAC coder
            // decorrelates the channels at 192k and that reads as a background
            // hiss on the near-mono voice recordings this mostly handles.
            //
            // It matters more here than there, because a conformed file is
            // encoded again on the way out -- so a VFR screen recording, which
            // is exactly what this path exists for, would otherwise carry two
            // generations of it.
            //
            // Copying the stream instead would be better still and is the
            // right end state: this rewrites the video cadence and has no
            // reason to touch the audio at all. Not done here because the
            // fallback loop below retries on *encoder* failure, and an audio
            // codec the container will not take would fail all four attempts
            // identically -- that needs its own fallback, not a one-word edit.
            "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart", "-y",
            tmp_s.as_str(),
        ]);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => continue,
        };
        // Drain the event stream so ffmpeg's progress spam on stderr can never
        // fill the pipe and stall a long transcode; then read the exit status.
        if let Ok(events) = child.iter() {
            for _ in events {}
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok && tmp.exists() {
            std::fs::rename(&tmp, &out)?;
            return Ok(out);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    anyhow::bail!("no working encoder to conform {}", src.display())
}

/// What [`remux_to_mkv`] names its copy, in the source's cache folder.
pub const REMUX_NAME: &str = "remux.mkv";

/// Copy `src` into Matroska without re-encoding the picture, for containers
/// with no seek index (`MediaEngine::needs_index`: MPEG-TS, MPEG-PS, FLV).
/// MKV carries one, so preview and export both seek exactly in the copy. A
/// stream copy takes seconds even for a long file, and the picture is
/// untouched. A prior copy is reused.
///
/// `+genpts` because a program stream (.vob) leaves some packets without a
/// timestamp, which Matroska refuses. Only picture and sound are copied: a
/// transport stream also carries data and subtitle streams that MKV can't
/// hold and nothing here edits. Sound MKV won't take as it is (Blu-ray's PCM)
/// is made FLAC instead, which is lossless.
pub fn remux_to_mkv(src: &Path) -> anyhow::Result<PathBuf> {
    ensure_ffmpeg()?;
    let dir = cache_dir(src)?;
    let out = dir.join(REMUX_NAME);
    if out.exists() {
        return Ok(out);
    }
    // a temp name first, so a crash never leaves a half-written copy that
    // later looks finished
    let tmp = dir.join("remux.partial.mkv");
    for audio in ["copy", "flac"] {
        let _ = std::fs::remove_file(&tmp);
        let mut cmd = FfmpegCommand::new();
        cmd.args(["-fflags", "+genpts"]);
        cmd.input(src.to_string_lossy());
        cmd.args(["-map", "0:v:0", "-map", "0:a?", "-c:v", "copy", "-c:a", audio, "-y"]);
        cmd.output(tmp.to_string_lossy());
        let Ok(mut child) = cmd.spawn() else { continue };
        // drain, so a long copy never stalls on a full stderr pipe
        if let Ok(events) = child.iter() {
            for _ in events {}
        }
        if child.wait().map(|s| s.success()).unwrap_or(false) && tmp.exists() {
            std::fs::rename(&tmp, &out)?;
            return Ok(out);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    anyhow::bail!("could not copy {} into MKV", src.display())
}

/// Decode any source's audio to a clean float WAV at `rate`, stereo. Used to
/// feed the music-separation model from a uniform, well-formed input — ffmpeg
/// normalises odd channel layouts and containers that the in-process decoder
/// chokes on. Drains the event stream so a long decode never stalls on a full
/// stderr pipe.
/// `start_s`/`dur_s` limit the decode to one span of the source (0/None = whole
/// file) — separation only ever needs the span a clip actually uses.
pub fn decode_audio_wav(
    src: &Path,
    out: &Path,
    rate: u32,
    start_s: f64,
    dur_s: Option<f64>,
) -> anyhow::Result<()> {
    ensure_ffmpeg()?;
    let src_s = src.to_string_lossy().to_string();
    let out_s = out.to_string_lossy().to_string();
    let rate_s = rate.to_string();
    let start_str = format!("{:.3}", start_s.max(0.0));
    let dur_str = dur_s.map(|d| format!("{:.3}", d.max(0.0)));
    let mut cmd = FfmpegCommand::new();
    // input-level seek so ffmpeg skips straight to the span
    if start_s > 0.0 {
        cmd.args(["-ss", start_str.as_str()]);
    }
    if let Some(d) = dur_str.as_deref() {
        cmd.args(["-t", d]);
    }
    cmd.input(src_s.as_str());
    cmd.args(["-vn", "-ac", "2", "-ar", rate_s.as_str(), "-c:a", "pcm_f32le", "-y", out_s.as_str()]);
    let mut child = cmd.spawn()?;
    if let Ok(events) = child.iter() {
        for _ in events {}
    }
    anyhow::ensure!(
        child.wait().map(|s| s.success()).unwrap_or(false),
        "audio decode failed for {}",
        src.display()
    );
    Ok(())
}

/// Where a "remove music" separation writes its vocals for one span of a
/// source: `vocals_<startMs>_<endMs>.wav` in the source's cache dir. Separating
/// only the span a clip uses keeps the job proportional to the edit rather than
/// to the whole recording (an hour-long source is otherwise ~40 min of work and
/// gigabytes of RAM to use five minutes of it).
pub fn vocals_range_path(src: &Path, start_s: f64, end_s: f64) -> anyhow::Result<PathBuf> {
    let a = (start_s.max(0.0) * 1000.0).round() as i64;
    let b = (end_s.max(start_s.max(0.0)) * 1000.0).round() as i64;
    Ok(cache_dir(src)?.join(format!("vocals_{a}_{b}.wav")))
}

/// The *raw* separated vocals for a span, before strength-blending and makeup
/// gain. Kept so changing the strength only re-mixes (fast) instead of re-running
/// the model. Named "vocalsraw_" so it can't be picked up by `vocals_covering`,
/// which matches the "vocals_" prefix.
pub fn vocals_raw_path(src: &Path, start_s: f64, end_s: f64) -> anyhow::Result<PathBuf> {
    let a = (start_s.max(0.0) * 1000.0).round() as i64;
    let b = (end_s.max(start_s.max(0.0)) * 1000.0).round() as i64;
    Ok(cache_dir(src)?.join(format!("vocalsraw_{a}_{b}.wav")))
}

/// Find a cached separation that fully covers `[start_s, end_s]`, returning its
/// path and the source time its first sample corresponds to (so callers can
/// offset into it). Falls back to the legacy whole-source `vocals.wav`. None =
/// nothing covers this span, so callers should use the original audio — a clip
/// trimmed outside its separated span quietly gets its music back rather than
/// the wrong audio.
pub fn vocals_covering(src: &Path, start_s: f64, end_s: f64) -> Option<(PathBuf, f64)> {
    let dir = cache_dir(src).ok()?;
    // legacy: a whole-source separation covers everything
    let legacy = dir.join("vocals.wav");
    if legacy.is_file() {
        return Some((legacy, 0.0));
    }
    const TOL_MS: i64 = 50; // float/rounding slack
    let want_a = (start_s.max(0.0) * 1000.0).round() as i64;
    let want_b = (end_s.max(start_s.max(0.0)) * 1000.0).round() as i64;
    let mut best: Option<(PathBuf, i64)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(mid) = name.strip_prefix("vocals_").and_then(|n| n.strip_suffix(".wav")) else {
            continue;
        };
        let Some((a, b)) = mid.split_once('_') else { continue };
        let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) else { continue };
        if a <= want_a + TOL_MS && b >= want_b - TOL_MS {
            // prefer the tightest covering span
            if best.as_ref().is_none_or(|(_, ba)| a > *ba) {
                best = Some((entry.path(), a));
            }
        }
    }
    best.map(|(p, a)| (p, a as f64 / 1000.0))
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
        kind: MediaKind::Video,
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

/// How long an untouched cache entry is kept, in days.
///
/// Long enough that a project you return to next week still opens instantly,
/// short enough that footage you imported once and forgot doesn't sit in temp
/// forever.
pub const CACHE_KEEP_DAYS: u64 = 14;

/// Delete cached proxies and frame-rate conversions nothing has touched
/// recently.
///
/// Every import leaves a folder here: scrub thumbnails, and for
/// variable-frame-rate sources a full re-encoded copy of the video. None of it
/// was ever removed. On the machine this was written on, ordinary use had
/// already left 267 MB in temp; someone importing screen recordings all week
/// would accumulate that per file, and nothing would ever give it back.
///
/// Safe to lose: anything deleted is rebuilt on next use, at the cost of
/// re-importing that one file. Best effort throughout — this runs at startup
/// and must never be the reason the app fails to open.
pub fn sweep_media_cache(max_age: std::time::Duration) -> u64 {
    sweep_cache_in(&std::env::temp_dir().join("cutlass-cache"), max_age)
}

/// The body of [`sweep_media_cache`], with the root exposed so a test can
/// exercise it without deleting the real cache off the running machine.
pub fn sweep_cache_in(root: &Path, max_age: std::time::Duration) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    let now = std::time::SystemTime::now();
    let mut freed = 0u64;

    for e in entries.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = e.path();
        // Age the folder by its NEWEST file: touching any part of a cache
        // entry means the media is still in use, and dropping it because one
        // thumbnail happens to be old would throw away the conform too.
        let newest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| f.metadata().ok())
            .filter_map(|m| m.modified().ok())
            .max();
        let Some(newest) = newest else { continue };
        let stale = now.duration_since(newest).is_ok_and(|age| age > max_age);
        if !stale {
            continue;
        }
        let size: u64 = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|f| f.metadata().ok())
            .map(|m| m.len())
            .sum();
        if std::fs::remove_dir_all(&dir).is_ok() {
            freed += size;
        }
    }
    freed
}

pub fn cache_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir()
        .join("cutlass-cache")
        .join(format!("{:016x}", path_hash(path)));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn read_frames(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    read_frames_named(dir, FULL_FRAME_PREFIX)
}

/// The quick first pass: far fewer frames, so the scrub strip has something in
/// it within about a second instead of a minute. Kept under its own prefix
/// because the two passes sample at different intervals — mixed together and
/// sorted they would interleave, and the strip maps position to thumbnail by
/// index, so every frame after the first would point at the wrong moment.
pub fn read_coarse_frames(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    read_frames_named(dir, COARSE_FRAME_PREFIX)
}

fn read_frames_named(dir: &Path, prefix: char) -> anyhow::Result<Vec<PathBuf>> {
    let mut frames: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jpg"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    frames.sort();
    Ok(frames)
}

#[cfg(test)]
mod heif_tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// A HEIF file's boxes, enough to say what to do to the picture: item
    /// `primary`, and properties ispe (1), irot 3 (2), imir 1 (3), with item 1
    /// given all three and item 2 (its thumbnail) only the turn.
    fn heif(primary: u16, wide: bool) -> Vec<u8> {
        let ipco = [bx(b"ispe", &[0; 12]), bx(b"irot", &[3]), bx(b"imir", &[1])].concat();
        let idx = |i: u8| if wide { vec![0x80, i] } else { vec![0x80 | i] };
        let mut ipma = vec![0, 0, 0, wide as u8, 0, 0, 0, 2];
        ipma.extend([0, 1, 3]);
        ipma.extend([idx(1), idx(2), idx(3)].concat());
        ipma.extend([0, 2, 1]);
        ipma.extend(idx(2));
        let iprp = bx(b"iprp", &[bx(b"ipco", &ipco), bx(b"ipma", &ipma)].concat());
        let pitm = bx(b"pitm", &[0, 0, 0, 0, (primary >> 8) as u8, primary as u8]);
        let meta = bx(b"meta", &[vec![0, 0, 0, 0], pitm, iprp].concat());
        [bx(b"ftyp", b"heicmif1"), meta].concat()
    }

    /// An iPhone photo held upright is stored on its side with 'irot' 3:
    /// three quarter turns anticlockwise. Shown without reading it, it was on
    /// its side in Cutlass.
    #[test]
    fn a_heif_picture_is_turned_and_mirrored_as_it_asks_in_order() {
        use HeifStep::*;
        assert_eq!(heif_transform(&heif(1, false)), vec![TurnAnticlockwise(3), Mirror(1)]);
        assert_eq!(heif_transform(&heif(1, true)), vec![TurnAnticlockwise(3), Mirror(1)], "16-bit indices");
    }

    /// Properties belong to items: the thumbnail's turn is not the photo's.
    #[test]
    fn only_the_primary_pictures_properties_count() {
        assert_eq!(heif_transform(&heif(2, false)), vec![HeifStep::TurnAnticlockwise(3)]);
        assert_eq!(heif_transform(&heif(9, false)), Vec::new(), "an item with nothing listed");
    }

    #[test]
    fn anything_else_asks_for_nothing() {
        assert_eq!(heif_transform(b"\x89PNG\r\n\x1a\n not heif"), Vec::new());
        assert_eq!(heif_transform(&[]), Vec::new());
        let truncated = heif(1, false);
        assert_eq!(heif_transform(&truncated[..truncated.len() - 3]), Vec::new(), "cut short");
    }

    #[test]
    fn heif_types_and_icons_are_the_ones_converted() {
        for p in ["a.heic", "B.HEIC", "c.heif", "d.hif", "e.avif", "f.ico"] {
            assert!(needs_conversion(p), "{p}");
            assert!(is_still(p), "{p} is a still");
        }
        for p in ["a.png", "b.jpg", "c.mp4"] {
            assert!(!needs_conversion(p), "{p}");
        }
    }
}

#[cfg(test)]
mod frame_pass_tests {
    use super::*;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"not really a jpeg").unwrap();
    }

    /// The two passes sample at different intervals, and the scrub strip finds
    /// a thumbnail by `floor(t * scrub_fps)`. If a reader ever returned both
    /// sets, the indices would run through the coarse frames first and every
    /// position in the clip would show the wrong moment — with no error, and
    /// nothing on screen to suggest the strip is lying.
    #[test]
    fn a_coarse_pass_and_a_full_pass_never_mix() {
        let dir = std::env::temp_dir().join(format!("cutlass-frames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for i in 1..=3 {
            touch(&dir, &format!("c{i:05}.jpg"));
        }
        for i in 1..=5 {
            touch(&dir, &format!("f{i:05}.jpg"));
        }
        touch(&dir, "cfr30.mp4"); // a conform result shares the folder
        touch(&dir, "notes.txt");

        let coarse = read_coarse_frames(&dir).unwrap();
        let full = read_frames(&dir).unwrap();
        assert_eq!(coarse.len(), 3, "coarse reader picked up something else");
        assert_eq!(full.len(), 5, "full reader picked up something else");
        assert!(
            coarse.iter().all(|p| !full.contains(p)),
            "the two sets overlap"
        );

        // Order is what maps a position in the clip to a frame.
        let names: Vec<String> = full
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "frames came back out of time order");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A clip short enough that the full pass is already quick should not pay
    /// for two passes, and one long enough to hurt should.
    #[test]
    fn only_long_clips_are_worth_splitting_in_two() {
        assert!(COARSE_PASS_ABOVE_S > 0.0);
        assert!(
            COARSE_SCRUB_FRAMES < MAX_SCRUB_FRAMES,
            "the quick pass must be smaller than the one it stands in for"
        );
        // 30 minutes of screen recording is the case this exists for.
        let long = 1800.0;
        let coarse_fps = COARSE_SCRUB_FRAMES / long;
        let full_fps = MAX_SCRUB_FRAMES / long;
        assert!(coarse_fps < full_fps);
        assert!(long > COARSE_PASS_ABOVE_S);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Import leaves a cache folder per file — thumbnails, and for a
    /// variable-frame-rate source a whole re-encoded copy of the video.
    /// Nothing removed any of it, so temp grew without bound.
    ///
    /// The sweep must take stale entries and leave everything else alone: a
    /// folder deleted while its project is open costs a re-import, and one
    /// left behind costs disk forever.
    #[test]
    fn stale_cache_is_swept_and_live_cache_is_not() {
        use std::time::Duration;
        // A private root: sweeping the machine's real cache from a test would
        // delete a developer's proxies mid-session.
        let root = std::env::temp_dir().join(format!("cutlass_sweeptest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let stale = root.join("aaaa");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("cfr30.mp4"), vec![0u8; 4096]).unwrap();

        let freed = sweep_cache_in(&root, Duration::ZERO);
        assert!(!stale.exists(), "a stale entry should be gone");
        assert!(freed >= 4096, "it should report what it reclaimed, got {freed}");

        // With the real threshold, a just-written entry survives — deleting
        // one while its project is open costs a re-import.
        let live = root.join("bbbb");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("f00001.jpg"), b"x").unwrap();
        assert_eq!(sweep_cache_in(&root, Duration::from_secs(CACHE_KEEP_DAYS * 86_400)), 0);
        assert!(live.exists(), "a cache entry in use must not be swept");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Runs at startup, so a missing or unreadable cache must not stop the
    /// app opening.
    #[test]
    fn sweeping_somewhere_that_does_not_exist_is_not_fatal() {
        assert_eq!(
            sweep_cache_in(Path::new("Z:/no/such/cache"), std::time::Duration::ZERO),
            0
        );
    }

    /// The "remove music" cache must only be used when it fully covers the span
    /// a clip asks for, and must report where its first sample sits in source
    /// time — a wrong offset would play audio from the wrong part of the clip.
    #[test]
    fn vocals_covering_matches_span_and_reports_start() {
        let src = std::env::temp_dir().join(format!("cutlass_vocals_test_{}.mp4", std::process::id()));
        std::fs::write(&src, b"x").unwrap();
        let dir = cache_dir(&src).unwrap();
        std::fs::write(dir.join("vocals_8000_12000.wav"), b"x").unwrap();

        // fully inside the separated span -> used, with its start reported
        let (path, start) = vocals_covering(&src, 9.0, 11.0).expect("span is covered");
        assert!(path.ends_with("vocals_8000_12000.wav"));
        assert!((start - 8.0).abs() < 1e-6, "start was {start}");

        // exact bounds still count
        assert!(vocals_covering(&src, 8.0, 12.0).is_some());

        // reaching outside it -> no coverage, so callers fall back to the
        // original audio rather than playing the wrong range
        assert!(vocals_covering(&src, 5.0, 11.0).is_none(), "starts before");
        assert!(vocals_covering(&src, 9.0, 15.0).is_none(), "ends after");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&src);
    }
}
