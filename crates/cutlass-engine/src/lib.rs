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

pub struct MediaEngine {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    stream_index: usize,
    time_base: f64,
    duration_s: f64,
    width: u32,
    height: u32,
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
        let decoder = ctx.decoder().video()?;
        let duration_s = ictx.duration() as f64 / AV_TIME_BASE as f64;
        let (width, height) = (decoder.width(), decoder.height());
        Ok(Self {
            ictx,
            decoder,
            stream_index,
            time_base,
            duration_s,
            width,
            height,
        })
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
                    self.decoder.send_packet(&packet)?;
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

    fn frame_pts_s(&self, frame: &ffmpeg::frame::Video) -> f64 {
        frame.pts().unwrap_or(0) as f64 * self.time_base
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
        let ts = (t.max(0.0) * AV_TIME_BASE as f64) as i64;
        let ret = unsafe {
            ffmpeg::ffi::av_seek_frame(
                self.ictx.as_mut_ptr(),
                -1,
                ts,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            )
        };
        if ret < 0 {
            return Err(anyhow!("av_seek_frame({t:.3}s) failed: {ret}"));
        }
        self.decoder.flush();
        Ok(())
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
        let t = t.clamp(0.0, self.duration_s.max(0.0));
        // av_seek_frame directly: Input::seek's avformat_seek_file form
        // fails with EPERM on some demuxers/states
        self.seek_raw(t)?;

        let mut last = None;
        while let Some(frame) = self.decode_next()? {
            let pts = self.frame_pts_s(&frame);
            let hit = !exact || pts + 1e-6 >= t;
            last = Some(frame);
            if hit {
                break;
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
