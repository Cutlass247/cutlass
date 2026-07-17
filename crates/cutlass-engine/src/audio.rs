//! Audio decode: any source stream → interleaved f32 stereo at a target
//! sample rate (the output device's), via libav + swresample.

use anyhow::{anyhow, Context as _};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::ffi::AV_TIME_BASE;
use ffmpeg::media::Type;
use ffmpeg::software::resampling;
use ffmpeg::util::channel_layout::ChannelLayout;
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
    eof: bool,
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
        let resampler = resampling::Context::get2(
            decoder.format(),
            decoder.ch_layout().clone(),
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
            eof: false,
        })
    }

    /// Coarse seek; decode resumes at the nearest packet before `t` and the
    /// caller-visible drift is at most one audio frame (~21 ms for AAC).
    pub fn seek(&mut self, t: f64) -> anyhow::Result<()> {
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
            return Err(anyhow!("audio seek({t:.3}) failed: {ret}"));
        }
        self.decoder.flush();
        self.eof = false;
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

    /// Next chunk of interleaved f32 stereo at the target rate. None = EOF.
    pub fn next_chunk(&mut self) -> anyhow::Result<Option<Vec<f32>>> {
        let Some(frame) = self.next_raw_frame()? else {
            return Ok(None);
        };
        let mut out = ffmpeg::frame::Audio::empty();
        self.resampler.run(&frame, &mut out)?;
        if out.samples() == 0 {
            return Ok(Some(Vec::new()));
        }
        let n = out.samples() * self.out_channels; // interleaved
        let bytes = &out.data(0)[..n * 4];
        let samples = bytes
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        Ok(Some(samples))
    }
}
