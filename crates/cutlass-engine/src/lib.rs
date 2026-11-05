//! Cutlass media engine — in-process libav decode.
//!
//! Spike B (spikes/media-engine/FINDINGS.md) proved the machine decodes
//! 4K H.264 at 236 fps but subprocess piping threw 75% of that away and
//! made seeks take ~857 ms. This crate is the fix: libav runs inside our
//! process, frames never cross a pipe, and seeks are av_seek_frame calls.
//!
//! Build requirements (Windows): FFMPEG_DIR → vendor/ffmpeg (shared LGPL
//! build), LIBCLANG_PATH → LLVM bin. Runtime needs vendor/ffmpeg/bin on
//! PATH (or the DLLs beside the exe).

pub mod audio;
pub mod player;
pub mod separate;
pub mod transcribe;

use anyhow::{anyhow, Context as _};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::ffi::AV_TIME_BASE;
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling;

/// A decoded frame, tightly packed RGBA.
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub pts_s: f64,
    pub data: Vec<u8>,
}

/// What a file holds, read from its header without opening a decoder.
#[derive(Debug, Clone, Copy)]
pub struct StreamProbe {
    /// A real picture stream. Album art does not count; see [`probe_streams`].
    pub has_video: bool,
    pub has_audio: bool,
    /// 0 when the container doesn't say.
    pub duration_s: f64,
}

/// Which kinds of stream a file has, and how long it is.
///
/// [`MediaEngine::open`] needs a picture, so a sound file has to be recognised
/// before it gets there. The catch is album art: MP3s and M4As very often carry
/// their cover as a one-frame video stream flagged `ATTACHED_PIC`. Counted as
/// footage, a song imports as a "video" whose picture ends after its first
/// frame, so those streams are skipped here.
pub fn probe_streams(path: &str) -> anyhow::Result<StreamProbe> {
    use ffmpeg::format::stream::Disposition;
    ffmpeg::init().context("ffmpeg init")?;
    let ictx = ffmpeg::format::input(&path).with_context(|| format!("open {path}"))?;
    let (mut has_video, mut has_audio) = (false, false);
    for s in ictx.streams() {
        match s.parameters().medium() {
            Type::Video if !s.disposition().contains(Disposition::ATTACHED_PIC) => has_video = true,
            Type::Audio => has_audio = true,
            _ => {}
        }
    }
    let d = ictx.duration();
    let duration_s = if d > 0 { d as f64 / AV_TIME_BASE as f64 } else { 0.0 };
    Ok(StreamProbe { has_video, has_audio, duration_s })
}

/// A picture stored as a grid of tiles, as libav reads it: for each tile its
/// stream index and where its top-left corner goes, then the crop of the
/// assembled grid that is the picture (x, y, width, height).
#[derive(Debug, Clone)]
pub struct TileGrid {
    pub tiles: Vec<(usize, i32, i32)>,
    pub crop: (i32, i32, i32, i32),
}

/// The tile grid of a HEIF picture (an iPhone's .heic is 35 tiles of 512),
/// or None for a picture that isn't one. libav parses the grid into a stream
/// group but neither it nor the ffmpeg command line assembles it, which is
/// what `cutlass_core::media::convert_picture` does with this.
pub fn tile_grid(path: &str) -> anyhow::Result<Option<TileGrid>> {
    use ffmpeg::ffi::AVStreamGroupParamsType;
    ffmpeg::init().context("ffmpeg init")?;
    let ictx = ffmpeg::format::input(&path).with_context(|| format!("open {path}"))?;
    // Safety: read-only walks of arrays libav sized itself (nb_stream_groups,
    // nb_tiles, a group's nb_streams), kept alive by `ictx` for the duration.
    unsafe {
        let fc = ictx.as_ptr();
        for g in 0..(*fc).nb_stream_groups as usize {
            let group = *(*fc).stream_groups.add(g);
            if group.is_null() || (*group).type_ != AVStreamGroupParamsType::TILE_GRID {
                continue;
            }
            let grid = (*group).params.tile_grid;
            if grid.is_null() {
                continue;
            }
            let mut tiles = Vec::with_capacity((*grid).nb_tiles as usize);
            for t in 0..(*grid).nb_tiles as usize {
                let o = *(*grid).offsets.add(t);
                anyhow::ensure!(o.idx < (*group).nb_streams, "tile {t} names a stream the group lacks");
                let stream = *(*group).streams.add(o.idx as usize);
                tiles.push(((*stream).index as usize, o.horizontal, o.vertical));
            }
            return Ok(Some(TileGrid {
                tiles,
                crop: (
                    (*grid).horizontal_offset,
                    (*grid).vertical_offset,
                    (*grid).width,
                    (*grid).height,
                ),
            }));
        }
    }
    Ok(None)
}

/// Where a file's own timestamps start, in seconds.
///
/// Every time this engine is given or returns counts from the start of the
/// clip, the way export's `ffmpeg -ss` counts it: ffmpeg adds this offset to
/// `-ss` before seeking. Most files start at 0, so treating raw timestamps as
/// clip time went unnoticed, but MPEG-TS (.ts, and camcorders' .mts/.m2ts)
/// starts around 1.4s: the preview showed a moment 1.4s earlier than the
/// export and could never reach the last 1.4s. 0 when the file doesn't say.
pub(crate) fn file_start_s(ictx: &ffmpeg::format::context::Input) -> f64 {
    let st = unsafe { (*ictx.as_ptr()).start_time };
    if st == ffmpeg::ffi::AV_NOPTS_VALUE || st < 0 {
        0.0
    } else {
        st as f64 / AV_TIME_BASE as f64
    }
}

pub struct MediaEngine {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    stream_index: usize,
    time_base: f64,
    /// Where the file's timestamps start, in seconds; see [`file_start_s`].
    start_s: f64,
    duration_s: f64,
    width: u32,
    height: u32,
    /// average frame rate (total frames / duration) and the nominal base rate.
    /// They match on constant-frame-rate sources and diverge on VFR ones
    /// (screen recordings), which is how `is_vfr` decides whether the clip
    /// needs conforming to CFR before editing.
    avg_fps: f64,
    r_fps: f64,
    /// streaming-playback cursor: last decoded frame held until the
    /// requested time passes it (avoids per-frame seeks during play).
    pending: Option<ffmpeg::frame::Video>,
    stream_pts: Option<f64>,
    /// A still picture (see [`MediaEngine::is_still`]): its one frame, decoded
    /// on first use and returned for any time.
    still: bool,
    still_frame: Option<ffmpeg::frame::Video>,
}

impl MediaEngine {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        ffmpeg::init().context("ffmpeg init")?;
        let ictx = ffmpeg::format::input(&path).with_context(|| format!("open {path}"))?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow!("no video stream in {path}"))?;
        let stream_index = stream.index();
        let time_base = f64::from(stream.time_base());
        let mut ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        // frame-threaded decode on all cores — libav defaults to 1 thread
        ctx.set_threading(ffmpeg::threading::Config {
            kind: ffmpeg::threading::Type::Frame,
            count: 0,
        });
        // frame rates: average (real cadence) vs nominal base. A rational
        // with a zero denominator means "unknown"; treat as 0 so is_vfr stays
        // conservative and never flags a source we can't measure.
        let rate_f64 = |r: ffmpeg::Rational| if r.denominator() != 0 { f64::from(r) } else { 0.0 };
        let avg_fps = rate_f64(stream.avg_frame_rate());
        let r_fps = rate_f64(stream.rate());
        let decoder = ctx.decoder().video()?;
        let duration_s = ictx.duration() as f64 / AV_TIME_BASE as f64;
        let (width, height) = (decoder.width(), decoder.height());
        let start_s = file_start_s(&ictx);
        // Single pictures come through an image demuxer ("image2" for JPEG,
        // "png_pipe", "webp_pipe" and the like) with no length; a GIF of one
        // frame reads as 0.04s of video.
        let name = ictx.format().name();
        let still = name == "image2"
            || name.ends_with("_pipe")
            || (name == "gif" && duration_s <= 0.05);
        Ok(Self {
            ictx,
            decoder,
            stream_index,
            time_base,
            start_s,
            duration_s,
            width,
            height,
            avg_fps,
            r_fps,
            pending: None,
            stream_pts: None,
            still,
            still_frame: None,
        })
    }

    /// True for a still picture. It has no time to seek in, so every frame
    /// asked of it is its one frame. Seeking an image demuxer found nothing
    /// at all: a JPEG returned no frame at 0.
    pub fn is_still(&self) -> bool {
        self.still
    }

    /// A still's frame, decoded once from the start (no seek).
    fn still_rgba(&mut self, max_width: u32) -> anyhow::Result<RgbaFrame> {
        if self.still_frame.is_none() {
            self.still_frame = self.decode_next()?;
        }
        let frame = self.still_frame.as_ref().ok_or_else(|| anyhow!("no picture in this file"))?;
        self.to_rgba(frame, max_width)
    }

    /// Average frames-per-second over the whole clip (0 when unknown).
    pub fn avg_fps(&self) -> f64 {
        self.avg_fps
    }

    /// The demuxer libav read the file with: "mpegts", "flv", "mpeg" (program
    /// stream: .mpg, .vob), "matroska,webm", "mov,mp4,m4a,3gp,3g2,mj2", ...
    pub fn container(&self) -> &'static str {
        self.ictx.format().name()
    }

    /// True for containers with no seek index, where ffmpeg's `-ss` (what
    /// export seeks with) lands on the wrong frame or none at all. Measured on
    /// the shipped ffmpeg, keyframes a second apart: in the last second of an
    /// MPEG-TS or MPEG-PS file it renders no frame, and an FLV of Sorenson
    /// video starts on a corrupt one. Every other container tested (MP4, MOV,
    /// MKV, WebM, AVI, WMV, 3GP, OGV, MXF, DV) seeks exactly. These get a
    /// stream copy into MKV, which has an index, to edit; see `remux_to_mkv`.
    pub fn needs_index(&self) -> bool {
        matches!(self.container(), "mpegts" | "mpeg" | "flv")
    }

    /// True when the source is variable-frame-rate enough to need conforming
    /// to constant frame rate before editing. CFR sources report equal average
    /// and base rates; VFR sources (screen recordings especially) diverge —
    /// their frames sit at irregular timestamps, which throws audio out of sync
    /// once the clip is cut and reassembled. A 5% gap is well clear of the
    /// sub-percent noise a clean CFR file can show from duration rounding.
    pub fn is_vfr(&self) -> bool {
        self.avg_fps > 0.5
            && self.r_fps > 0.5
            && (self.r_fps - self.avg_fps).abs() > 0.05 * self.r_fps.max(self.avg_fps)
    }

    pub fn duration_s(&self) -> f64 {
        self.duration_s
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Decode the next frame in presentation order (raw, decoder-native
    /// pixel format). Returns None at end of stream.
    fn decode_next(&mut self) -> anyhow::Result<Option<ffmpeg::frame::Video>> {
        let mut frame = ffmpeg::frame::Video::empty();
        loop {
            if self.decoder.receive_frame(&mut frame).is_ok() {
                return Ok(Some(frame));
            }
            let mut sent = false;
            for r in self.ictx.packets() {
                let (stream, packet) = r?;
                if stream.index() == self.stream_index {
                    // A damaged packet (where a seek into a stream with no
                    // index lands mid-frame) is skipped, as ffmpeg does,
                    // rather than ending the decode there.
                    match self.decoder.send_packet(&packet) {
                        Ok(()) => {}
                        Err(ffmpeg::Error::InvalidData) => continue,
                        Err(e) => return Err(e.into()),
                    }
                    sent = true;
                    break;
                }
            }
            if !sent {
                // no more packets: drain the decoder
                self.decoder.send_eof().ok();
                return Ok(if self.decoder.receive_frame(&mut frame).is_ok() {
                    Some(frame)
                } else {
                    None
                });
            }
        }
    }

    /// A frame's time in the clip (from its start, not the file's timestamps).
    /// libav's best-effort timestamp, which fills in where a container leaves
    /// `pts` out; that is also what ffmpeg itself times frames by.
    fn frame_pts_s(&self, frame: &ffmpeg::frame::Video) -> f64 {
        frame.timestamp().or_else(|| frame.pts()).unwrap_or(0) as f64 * self.time_base - self.start_s
    }

    /// Sequential decode of every remaining frame; returns count.
    /// (Benchmark path — playback will stream instead of draining.)
    pub fn drain(&mut self) -> anyhow::Result<u64> {
        let mut n = 0;
        while self.decode_next()?.is_some() {
            n += 1;
        }
        Ok(n)
    }

    fn seek_raw(&mut self, t: f64) -> anyhow::Result<()> {
        let ts = ((t.max(0.0) + self.start_s) * AV_TIME_BASE as f64) as i64;
        let seek = |ctx: &mut ffmpeg::format::context::Input, flags| unsafe {
            ffmpeg::ffi::av_seek_frame(ctx.as_mut_ptr(), -1, ts, flags)
        };
        // Backward finds the keyframe at or before `ts`. A file whose first
        // keyframe comes after it has none (an FLV's first video frame is
        // often a few ms in, behind its audio), and is refused; forward then
        // finds the first one, which is the frame that was wanted.
        let mut ret = seek(&mut self.ictx, ffmpeg::ffi::AVSEEK_FLAG_BACKWARD);
        if ret < 0 {
            ret = seek(&mut self.ictx, 0);
        }
        if ret < 0 {
            return Err(anyhow!("av_seek_frame({t:.3}s) failed: {ret}"));
        }
        self.decoder.flush();
        // any seek invalidates the streaming cursor
        self.pending = None;
        self.stream_pts = None;
        Ok(())
    }

    /// Seek so decoding resumes at or before `t`, and return the first frame
    /// decoded there (None only past the end).
    ///
    /// Files with an index (MP4, MKV, MOV) seek to the keyframe before `t`.
    /// MPEG-TS, and FLV without a keyframe index (OBS writes them without),
    /// land on whatever packet is nearest instead, and the decoder skips
    /// ahead to the next keyframe, which can be seconds after `t`. When the
    /// first frame comes back late, seek again earlier, doubling the margin,
    /// until it doesn't or the seek reaches the start.
    fn seek_landing_before(&mut self, t: f64) -> anyhow::Result<Option<ffmpeg::frame::Video>> {
        let mut back = 0.0;
        loop {
            let target = (t - back).max(0.0);
            self.seek_raw(target)?;
            let first = self.decode_next()?;
            let late = match &first {
                Some(f) => self.frame_pts_s(f) > t + 1e-3,
                None => true,
            };
            if !late || target <= 0.0 {
                return Ok(first);
            }
            back = if back == 0.0 { 0.5 } else { back * 2.0 };
        }
    }

    /// Streaming playback access: return the frame current at time `t`.
    /// Decodes forward from the last position (no seek) when `t` advances
    /// normally, holding a frame until `t` passes it; only seeks on a
    /// backward or large-forward jump. This is the fast path that makes
    /// real-time playback possible — the 261 fps decode headroom.
    pub fn stream_frame_at(&mut self, t: f64, max_width: u32) -> anyhow::Result<RgbaFrame> {
        if self.still {
            return self.still_rgba(max_width);
        }
        let t = t.clamp(0.0, self.duration_s.max(0.0));
        let need_seek = match self.stream_pts {
            None => true,
            Some(p) => t < p - 0.05 || t > p + 1.0,
        };
        if need_seek {
            let first = self.seek_landing_before(t)?;
            self.stream_pts = first.as_ref().map(|f| self.frame_pts_s(f));
            self.pending = first;
        }
        loop {
            let have = self.pending.as_ref().map(|f| self.frame_pts_s(f));
            if let Some(p) = have {
                if p + 1e-6 >= t {
                    break; // current held frame still covers t
                }
            }
            match self.decode_next()? {
                Some(f) => {
                    self.stream_pts = Some(self.frame_pts_s(&f));
                    self.pending = Some(f);
                }
                None => break, // EOF: return the last held frame
            }
        }
        let frame = self
            .pending
            .as_ref()
            .ok_or_else(|| anyhow!("no frame at t={t}"))?;
        self.to_rgba(frame, max_width)
    }

    /// Sequentially decode the whole file, emitting one RGBA frame per
    /// `every_s` seconds of source time (the scrub-proxy generator: one
    /// fast pass, no per-sample seeking). Returns the emitted count.
    pub fn sample_frames(
        &mut self,
        every_s: f64,
        max_width: u32,
        mut cb: impl FnMut(RgbaFrame) -> anyhow::Result<()>,
    ) -> anyhow::Result<u32> {
        anyhow::ensure!(every_s > 0.0, "every_s must be positive");
        if self.still {
            cb(self.still_rgba(max_width)?)?;
            return Ok(1);
        }
        self.seek_raw(0.0)?;
        let mut next_t = 0.0f64;
        let mut emitted = 0u32;
        while let Some(frame) = self.decode_next()? {
            if self.frame_pts_s(&frame) + 1e-6 >= next_t {
                cb(self.to_rgba(&frame, max_width)?)?;
                emitted += 1;
                next_t += every_s;
            }
        }
        Ok(emitted)
    }

    /// Frame-accurate random access: keyframe-seek before `t`, then decode
    /// forward until the first frame with pts >= t, scaled to RGBA at most
    /// `max_width` wide (0 = native). On long-GOP sources this costs up to
    /// a full GOP of decode — use all-intra proxies or `keyframe_at` when
    /// latency matters.
    pub fn frame_at(&mut self, t: f64, max_width: u32) -> anyhow::Result<RgbaFrame> {
        self.seek_frame(t, max_width, true)
    }

    /// Fast random access: nearest keyframe at or before `t`. One frame of
    /// decode regardless of GOP length — the scrub path for raw sources.
    pub fn keyframe_at(&mut self, t: f64, max_width: u32) -> anyhow::Result<RgbaFrame> {
        self.seek_frame(t, max_width, false)
    }

    fn seek_frame(&mut self, t: f64, max_width: u32, exact: bool) -> anyhow::Result<RgbaFrame> {
        if self.still {
            return self.still_rgba(max_width);
        }
        let t = t.clamp(0.0, self.duration_s.max(0.0));
        // av_seek_frame directly: Input::seek's avformat_seek_file form
        // fails with EPERM on some demuxers/states
        let mut last = self.seek_landing_before(t)?;
        if exact {
            while last.as_ref().is_some_and(|f| self.frame_pts_s(f) + 1e-6 < t) {
                match self.decode_next()? {
                    Some(frame) => last = Some(frame),
                    None => break, // past the last frame: that one is nearest
                }
            }
        }
        let frame = last.ok_or_else(|| anyhow!("no frame at t={t}"))?;
        self.to_rgba(&frame, max_width)
    }

    fn to_rgba(&self, frame: &ffmpeg::frame::Video, max_width: u32) -> anyhow::Result<RgbaFrame> {
        let (w, h) = if max_width > 0 && frame.width() > max_width {
            let h = (frame.height() as u64 * max_width as u64 / frame.width() as u64) as u32;
            (max_width, h & !1)
        } else {
            (frame.width(), frame.height())
        };
        let mut scaler = scaling::Context::get(
            frame.format(),
            frame.width(),
            frame.height(),
            Pixel::RGBA,
            w,
            h,
            scaling::Flags::BILINEAR,
        )?;
        let mut rgba = ffmpeg::frame::Video::empty();
        scaler.run(frame, &mut rgba)?;

        // copy out row-by-row: libav pads stride
        let stride = rgba.stride(0);
        let row = (w * 4) as usize;
        let src = rgba.data(0);
        let mut data = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            data.extend_from_slice(&src[y * stride..y * stride + row]);
        }
        Ok(RgbaFrame {
            width: w,
            height: h,
            pts_s: self.frame_pts_s(frame),
            data,
        })
    }
}
