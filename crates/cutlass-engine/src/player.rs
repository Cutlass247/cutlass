//! Timeline audio playback.
//!
//! Three actors:
//! - decode thread: walks the timeline (clips + gaps→silence), decodes and
//!   resamples audio, pushes interleaved stereo f32 into a bounded ring
//! - cpal thread: owns the (!Send) output stream; its callback drains the
//!   ring and counts consumed samples — **the sample counter IS the
//!   transport clock**
//! - the app: holds a `PlaybackHandle` (Send), polls `clock()`, calls
//!   `stop()`
//!
//! The audio callback never blocks: it try-locks the ring and emits
//! silence on contention or underrun.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context as _};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::AudioDecoder;

/// What the timeline sounds like: one entry per clip on the audio track.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AudioClip {
    pub path: String,
    pub start: f64,
    pub len: f64,
    pub src_in: f64,
}

struct Shared {
    ring: Mutex<VecDeque<f32>>, // interleaved stereo
    consumed: AtomicU64,        // stereo sample-pairs played out
    decode_done: AtomicBool,
    stopped: AtomicBool,
    sample_rate: u32,
    base_t: f64,
}

pub struct PlaybackHandle {
    shared: Arc<Shared>,
}

impl PlaybackHandle {
    /// Timeline seconds, driven by samples actually handed to the device.
    pub fn clock(&self) -> f64 {
        self.shared.base_t
            + self.shared.consumed.load(Ordering::Relaxed) as f64
                / self.shared.sample_rate as f64
    }

    pub fn ended(&self) -> bool {
        self.shared.decode_done.load(Ordering::Relaxed)
            && self.shared.ring.lock().map(|r| r.is_empty()).unwrap_or(true)
    }

    pub fn stop(&self) -> f64 {
        self.shared.stopped.store(true, Ordering::Relaxed);
        self.clock()
    }
}

/// Start playing `clips` from timeline position `from_t`.
pub fn start(mut clips: Vec<AudioClip>, from_t: f64) -> anyhow::Result<PlaybackHandle> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no audio output device"))?;
    let config = device.default_output_config().context("output config")?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        anyhow::bail!("unsupported device sample format {:?}", config.sample_format());
    }
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    let shared = Arc::new(Shared {
        ring: Mutex::new(VecDeque::with_capacity(sample_rate as usize * 4)),
        consumed: AtomicU64::new(0),
        decode_done: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        sample_rate,
        base_t: from_t,
    });

    clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

    // ── decode thread ───────────────────────────────────────────────────
    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("cutlass-audio-decode".into())
            .spawn(move || {
                if let Err(e) = decode_loop(&clips, from_t, &shared) {
                    eprintln!("audio decode: {e:#}");
                }
                shared.decode_done.store(true, Ordering::Relaxed);
            })?;
    }

    // ── cpal thread (owns the !Send stream) ─────────────────────────────
    {
        let shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("cutlass-audio-out".into())
            .spawn(move || {
                let cb_shared = Arc::clone(&shared);
                let stream = device.build_output_stream(
                    &config.into(),
                    move |out: &mut [f32], _| fill_output(out, channels, &cb_shared),
                    |e| eprintln!("audio stream error: {e}"),
                    None,
                );
                let started = match stream {
                    Ok(s) => s.play().map(|_| s).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                match started {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        while !shared.stopped.load(Ordering::Relaxed)
                            && !(shared.decode_done.load(Ordering::Relaxed)
                                && shared.ring.lock().map(|r| r.is_empty()).unwrap_or(true))
                        {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })?;
        ready_rx
            .recv()
            .context("audio thread died")?
            .map_err(|e| anyhow!("audio stream: {e}"))?;
    }

    Ok(PlaybackHandle { shared })
}

fn fill_output(out: &mut [f32], channels: usize, shared: &Shared) {
    out.fill(0.0);
    let Ok(mut ring) = shared.ring.try_lock() else {
        return; // contention → one buffer of silence, never a blocked callback
    };
    let mut played = 0u64;
    for frame in out.chunks_mut(channels) {
        let (Some(l), Some(r)) = (ring.pop_front(), ring.pop_front()) else {
            break; // underrun → silence
        };
        frame[0] = l;
        if channels > 1 {
            frame[1] = r;
        }
        played += 1;
    }
    shared.consumed.fetch_add(played, Ordering::Relaxed);
}

fn decode_loop(clips: &[AudioClip], from_t: f64, shared: &Shared) -> anyhow::Result<()> {
    let rate = shared.sample_rate;
    let max_ring = rate as usize * 2 * 2; // 2 s of stereo
    let mut t = from_t;

    let push = |samples: &[f32], shared: &Shared| -> bool {
        let mut offset = 0;
        while offset < samples.len() {
            if shared.stopped.load(Ordering::Relaxed) {
                return false;
            }
            let mut ring = shared.ring.lock().unwrap();
            let room = max_ring.saturating_sub(ring.len());
            if room == 0 {
                drop(ring);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let n = room.min(samples.len() - offset);
            ring.extend(&samples[offset..offset + n]);
            offset += n;
        }
        true
    };

    loop {
        if shared.stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        let active = clips
            .iter()
            .find(|c| t >= c.start - 1e-9 && t < c.start + c.len - 1e-9);
        match active {
            Some(clip) => {
                let clip_end = clip.start + clip.len;
                match AudioDecoder::open(&clip.path, rate) {
                    Ok(mut dec) => {
                        dec.seek(t - clip.start + clip.src_in)?;
                        while t < clip_end - 1e-9 {
                            if shared.stopped.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            let Some(chunk) = dec.next_chunk()? else { break };
                            let remaining =
                                (((clip_end - t) * rate as f64) as usize).saturating_mul(2);
                            let take = chunk.len().min(remaining);
                            if !push(&chunk[..take], shared) {
                                return Ok(());
                            }
                            t += (take / 2) as f64 / rate as f64;
                        }
                    }
                    Err(_) => {} // no audio stream → fall through to silence
                }
                if t < clip_end - 1e-9 {
                    // source audio ran short (or none): silence to clip end
                    if !push_silence(clip_end - t, rate, shared, &push) {
                        return Ok(());
                    }
                }
                t = clip_end;
            }
            None => {
                let next = clips
                    .iter()
                    .map(|c| c.start)
                    .filter(|s| *s > t + 1e-9)
                    .fold(f64::INFINITY, f64::min);
                if !next.is_finite() {
                    return Ok(()); // past the last clip
                }
                if !push_silence(next - t, rate, shared, &push) {
                    return Ok(());
                }
                t = next;
            }
        }
    }
}

fn push_silence(
    seconds: f64,
    rate: u32,
    shared: &Shared,
    push: &dyn Fn(&[f32], &Shared) -> bool,
) -> bool {
    let mut left = (seconds.max(0.0) * rate as f64) as usize * 2;
    let zeros = vec![0f32; 4800 * 2];
    while left > 0 {
        let n = left.min(zeros.len());
        if !push(&zeros[..n], shared) {
            return false;
        }
        left -= n;
    }
    true
}
