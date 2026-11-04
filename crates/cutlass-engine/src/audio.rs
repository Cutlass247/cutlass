//! Audio decode: any source stream → interleaved f32 stereo at a target
//! sample rate (the output device's), via libav + swresample.

use anyhow::{anyhow, Context as _};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::media::Type;
use ffmpeg::software::resampling;
use ffmpeg::util::channel_layout::{ChannelLayout, ChannelLayoutMask};
use ffmpeg::util::format::sample::{Sample, Type as SampleType};

/// Normalized peak waveform (~`buckets` values) decoded in-process at
/// 8 kHz mono. Empty if the file has no (usable) audio.
pub fn waveform_peaks(path: &str, buckets: usize) -> Vec<f32> {
    let Ok(mut dec) = AudioDecoder::open_mono(path, 8_000) else {
        return Vec::new();
    };
    let mut samples: Vec<f32> = Vec::new();
    while let Ok(Some(chunk)) = dec.next_chunk() {
        if chunk.is_empty() && samples.len() > 8_000 * 3600 {
            break; // safety valve
        }
        samples.extend(chunk);
    }
    if samples.len() < 800 {
        return Vec::new();
    }
    let bucket = (samples.len() / buckets.max(1)).max(80);
    let mut peaks = Vec::with_capacity(samples.len() / bucket + 1);
    let mut peak = 0.0f32;
    for (i, s) in samples.iter().enumerate() {
        peak = peak.max(s.abs());
        if (i + 1) % bucket == 0 {
            peaks.push(peak);
            peak = 0.0;
        }
    }
    let max = peaks.iter().fold(1e-6f32, |m, p| m.max(*p));
    peaks.iter().map(|p| p / max).collect()
}

/// The channel layout to resample from.
///
/// Some sources carry a channel count with no speaker mask: every WMA file
/// (and so the sound of every WMV), raw WAV, a few odd captures. swr's Rust
/// wrapper unwraps that mask and panics, so such a stream is given the
/// standard layout for its channel count. Its frames still say "unspecified",
/// though, and swr refuses any frame that doesn't match what it was set up
/// with ("Input changed"), so `next_chunk` relabels them with this too.
/// Without that second half, those files decoded to nothing: silent in
/// playback, no waveform, nothing to transcribe.
fn usable_layout(l: &ChannelLayout) -> ChannelLayout<'static> {
    if let Some(owned) = l.mask().and_then(ChannelLayout::from_mask) {
        return owned;
    }
    let standard = ChannelLayout::default_for_channels(l.channels().max(1));
    if standard.mask().is_some() {
        standard
    } else if l.channels() >= 2 {
        ChannelLayout::STEREO
    } else {
        ChannelLayout::MONO
    }
}

/// [`waveform_peaks`] for a sound file, and its length as decoded.
///
/// Streams the peaks rather than holding every sample (an hour at 8 kHz is
/// 115 MB of them), sizing buckets from `approx_s`, the container's idea of
/// the length; how many come back doesn't matter, as the timeline spreads
/// them over the clip. The decoded length is exact where that idea isn't: a
/// raw .aac, or an MP3 with no length in its header, has it guessed from the
/// bitrate, and a recording that starts quietly came out at 29s for 3s.
pub fn waveform_and_length(path: &str, buckets: usize, approx_s: f64) -> (Vec<f32>, f64) {
    let Ok(mut dec) = AudioDecoder::open_mono(path, 8_000) else {
        return (Vec::new(), 0.0);
    };
    let bucket = ((approx_s.max(0.0) * 8_000.0) as usize / buckets.max(1)).max(80);
    let (mut peaks, mut peak, mut n) = (Vec::new(), 0.0f32, 0usize);
    while let Ok(Some(chunk)) = dec.next_chunk() {
        for s in chunk {
            peak = peak.max(s.abs());
            n += 1;
            if n % bucket == 0 {
                peaks.push(peak);
                peak = 0.0;
            }
        }
    }
    let len = n as f64 / 8_000.0;
    if n < 800 {
        return (Vec::new(), len);
    }
    let max = peaks.iter().fold(1e-6f32, |m, p| m.max(*p));
    (peaks.iter().map(|p| p / max).collect(), len)
}

pub struct AudioDecoder {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: resampling::Context,
    /// What the resampler was set up with; see [`usable_layout`].
    src_layout: ChannelLayout<'static>,
    stream_index: usize,
    time_base: f64,
    /// Where the file's timestamps start; seeks count from the clip's start,
    /// the same as the picture's (see `crate::file_start_s`).
    start_s: f64,
    out_channels: usize,
    out_rate: u32,
    eof: bool,
    drained: bool, // resampler tail flushed after input EOF
    /// The frame a seek landed in, and how many of its samples come before
    /// the seek point; `next_chunk` plays it from there. See `seek`.
    held: Option<ffmpeg::frame::Audio>,
    skip_in: usize,
}

/// A frame's best timestamp: libav's best-effort guess where the container
/// left `pts` out (program streams do), else `pts`.
fn frame_ts(frame: &ffmpeg::frame::Audio) -> Option<i64> {
    frame.timestamp().or_else(|| frame.pts())
}

impl AudioDecoder {
    /// Errors if the file has no audio stream — callers treat that as silence.
    pub fn open(path: &str, target_rate: u32) -> anyhow::Result<Self> {
        Self::open_with(path, target_rate, 2)
    }

    /// Mono variant (e.g. 16 kHz for whisper).
    pub fn open_mono(path: &str, target_rate: u32) -> anyhow::Result<Self> {
        Self::open_with(path, target_rate, 1)
    }

    fn open_with(path: &str, target_rate: u32, out_channels: usize) -> anyhow::Result<Self> {
        ffmpeg::init().context("ffmpeg init")?;
        let ictx = ffmpeg::format::input(&path).with_context(|| format!("open {path}"))?;
        let stream = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| anyhow!("no audio stream in {path}"))?;
        let stream_index = stream.index();
        let time_base = f64::from(stream.time_base());
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;
        let src_layout = usable_layout(&decoder.ch_layout());
        let start_s = crate::file_start_s(&ictx);
        let resampler = resampling::Context::get2(
            decoder.format(),
            src_layout.clone(),
            decoder.rate(),
            Sample::F32(SampleType::Packed),
            if out_channels == 1 {
                ChannelLayout::MONO
            } else {
                ChannelLayout::STEREO
            },
            target_rate,
        )?;
        Ok(Self {
            ictx,
            decoder,
            resampler,
            src_layout,
            stream_index,
            time_base,
            start_s,
            out_channels,
            out_rate: target_rate,
            eof: false,
            drained: false,
            held: None,
            skip_in: 0,
        })
    }

    /// Coarse seek; decode resumes at the nearest packet before `t` and the
    /// caller-visible drift is at most one audio frame (~21 ms for AAC).
    /// Seeks by the AUDIO stream in its own time base — seeking by the container
    /// default (the video stream) can land far from `t` when video keyframes are
    /// sparse, which garbled audio-offset seeks on some sources.
    pub fn seek(&mut self, t: f64) -> anyhow::Result<()> {
        let ts = ((t.max(0.0) + self.start_s) / self.time_base).round() as i64;
        let ret = unsafe {
            ffmpeg::ffi::av_seek_frame(
                self.ictx.as_mut_ptr(),
                self.stream_index as i32,
                ts,
                ffmpeg::ffi::AVSEEK_FLAG_BACKWARD,
            )
        };
        if ret < 0 {
            return Err(anyhow!("audio seek({t:.3}) failed: {ret}"));
        }
        self.decoder.flush();
        self.eof = false;
        self.drained = false;
        self.held = None;
        self.skip_in = 0;
        // Drop frames wholly before t, then keep the one t falls in, minus
        // its samples before t. This used to drop that frame as well, so
        // sound after every seek began up to a frame late: 21 ms of AAC, a
        // tenth of a second of FLAC, and a whole WavPack block, which can
        // run to a second.
        let rate = self.decoder.rate().max(1) as f64;
        loop {
            let Some(frame) = self.next_raw_frame()? else { break };
            let Some(ts) = frame_ts(&frame) else {
                // no timestamp to judge by: play from here rather than drop
                // what may be the rest of the file
                self.held = Some(frame);
                break;
            };
            let pts = ts as f64 * self.time_base - self.start_s;
            let dur = frame.samples() as f64 / rate;
            if pts + dur > t {
                let before = ((t - pts) * rate).round().max(0.0) as usize;
                self.skip_in = before.min(frame.samples());
                self.held = Some(frame);
                break;
            }
        }
        Ok(())
    }

    fn next_raw_frame(&mut self) -> anyhow::Result<Option<ffmpeg::frame::Audio>> {
        let mut frame = ffmpeg::frame::Audio::empty();
        loop {
            if self.decoder.receive_frame(&mut frame).is_ok() {
                return Ok(Some(frame));
            }
            if self.eof {
                return Ok(None);
            }
            let mut sent = false;
            for r in self.ictx.packets() {
                let (stream, packet) = r?;
                if stream.index() == self.stream_index {
                    // One damaged packet must not end the decode: a seek in a
                    // program stream (.mpg, .vob) lands mid-frame, and the
                    // partial frame it starts on is invalid. ffmpeg skips it
                    // too. Ending here silenced the clip from that point on.
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
                self.eof = true;
                self.decoder.send_eof().ok();
            }
        }
    }

    fn out_layout(&self) -> ChannelLayoutMask {
        if self.out_channels == 1 {
            ChannelLayoutMask::MONO
        } else {
            ChannelLayoutMask::STEREO
        }
    }

    fn pack(&self, out: &ffmpeg::frame::Audio) -> Vec<f32> {
        if out.samples() == 0 {
            return Vec::new();
        }
        let n = out.samples() * self.out_channels; // interleaved
        // Take what the frame reports, but never past what it actually holds:
        // a slice out of range here is a panic on the decode thread, which
        // takes audio down for good. chunks_exact drops any partial tail.
        let data = out.data(0);
        data[..(n * 4).min(data.len())]
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    /// Next chunk of interleaved f32 stereo at the target rate. None = EOF.
    pub fn next_chunk(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        let next = match self.held.take() {
            Some(f) => Some(f),
            None => self.next_raw_frame()?,
        };
        let Some(mut frame) = next else {
            // Input ended: drain the resampler's buffered tail once. Without
            // this, up-sampled audio (in_rate != target) loses its last
            // ~(target/in - 1) fraction — the "sound cuts off" at clip ends.
            if self.drained {
                return Ok(None);
            }
            self.drained = true;
            let mut out = ffmpeg::frame::Audio::empty();
            unsafe { out.alloc(Sample::F32(SampleType::Packed), 1 << 15, self.out_layout()) };
            self.resampler.flush(&mut out)?;
            return Ok(Some(self.pack(&out)));
        };
        // Size the output for the rate ratio (+headroom) so swr returns every
        // sample now. Leaving the frame empty makes the library size it to the
        // INPUT sample count, so up-sampling buffers the surplus every frame and
        // eventually drops the accumulated tail at EOF.
        let in_rate = self.decoder.rate().max(1);
        let cap =
            (frame.samples() as u64 * self.out_rate as u64 / in_rate as u64) as usize + 1024;
        let mut out = ffmpeg::frame::Audio::empty();
        unsafe { out.alloc(Sample::F32(SampleType::Packed), cap, self.out_layout()) };
        // Same layout the resampler was set up with, or it refuses the frame.
        // An unspecified layout owns no memory, so replacing it leaks nothing.
        if frame.ch_layout().mask().is_none() {
            frame.set_ch_layout(self.src_layout.clone());
        }
        self.resampler.run(&frame, &mut out)?;
        let mut chunk = self.pack(&out);
        // The part of a seek's frame before the seek point, at the output rate.
        if self.skip_in > 0 {
            let frames = (self.skip_in as u64 * self.out_rate as u64 / in_rate as u64) as usize;
            chunk.drain(..(frames * self.out_channels).min(chunk.len()));
            self.skip_in = 0;
        }
        Ok(Some(chunk))
    }
}
