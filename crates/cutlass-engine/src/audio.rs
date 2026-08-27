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

pub struct AudioDecoder {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: resampling::Context,
    stream_index: usize,
    time_base: f64,
    out_channels: usize,
    out_rate: u32,
    eof: bool,
    drained: bool, // resampler tail flushed after input EOF
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
        // Some containers (raw WAV, a few odd captures) report a channel count
        // with no layout mask; swr's Rust wrapper unwraps that mask and panics.
        // Substitute the standard mono/stereo layout for the channel count so
        // any source decodes instead of crashing the separation/playback.
        let src_layout = {
            let l = decoder.ch_layout();
            if l.mask().is_some() {
                l.clone()
            } else if l.channels() >= 2 {
                ChannelLayout::STEREO
            } else {
                ChannelLayout::MONO
            }
        };
        let resampler = resampling::Context::get2(
            decoder.format(),
            src_layout,
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
            stream_index,
            time_base,
            out_channels,
            out_rate: target_rate,
            eof: false,
            drained: false,
        })
    }

    /// Coarse seek; decode resumes at the nearest packet before `t` and the
    /// caller-visible drift is at most one audio frame (~21 ms for AAC).
    /// Seeks by the AUDIO stream in its own time base — seeking by the container
    /// default (the video stream) can land far from `t` when video keyframes are
    /// sparse, which garbled audio-offset seeks on some sources.
    pub fn seek(&mut self, t: f64) -> anyhow::Result<()> {
        let ts = (t.max(0.0) / self.time_base).round() as i64;
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
        // drop frames wholly before t
        loop {
            let Some(frame) = self.next_raw_frame()? else { break };
            let pts = frame.pts().unwrap_or(0) as f64 * self.time_base;
            let dur = frame.samples() as f64 / self.decoder.rate().max(1) as f64;
            if pts + dur >= t {
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
                    self.decoder.send_packet(&packet)?;
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
        out.data(0)[..n * 4]
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    /// Next chunk of interleaved f32 stereo at the target rate. None = EOF.
    pub fn next_chunk(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        let Some(frame) = self.next_raw_frame()? else {
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
        self.resampler.run(&frame, &mut out)?;
        Ok(Some(self.pack(&out)))
    }
}
