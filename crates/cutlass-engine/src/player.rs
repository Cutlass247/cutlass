//! Timeline audio playback.
//!
//! Three actors:
//! - decode thread: walks the timeline (clips + gapsâ†’silence), decodes and
//!   resamples audio, pushes interleaved stereo f32 into a bounded ring
//! - cpal thread: owns the (!Send) output stream; its callback drains the
//!   ring and counts consumed samples â€” **the sample counter IS the
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
    /// per-clip gain (Effect Controls); defaults to unity when absent.
    #[serde(default = "unity_gain")]
    pub volume: f64,
    /// retime factor: source seconds consumed per timeline second (2 =
    /// fast/high-pitched, 0.5 = slow/low-pitched). Live preview is a naive
    /// resample (pitch follows speed); export is atempo pitch-corrected.
    #[serde(default = "unity_gain")]
    pub speed: f64,
    /// seconds to slip this clip's audio against its video (+ later, - earlier)
    /// to correct baked-in A/V sync; 0 when absent.
    #[serde(default)]
    pub audio_offset: f64,
    /// fade in and out, seconds, as export's (linear); 0 when absent
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    /// volume keyframes, (clip time, gain), sorted; empty = `volume` all
    /// through. Played back as export renders them; both used to be heard
    /// only in the export, so a music bed dipped under a voice, or faded out,
    /// played at full here.
    #[serde(default)]
    pub volume_kf: Vec<(f64, f64)>,
}

impl AudioClip {
    /// The gain `u` seconds into the clip's sound, as export's chain applies
    /// it: volume (keyframed or not) and fades, in the clip's own time, which
    /// its sync offset then slides (so this is evaluated at `u - offset`).
    pub fn gain_at(&self, u: f64) -> f64 {
        let u = u - self.audio_offset;
        let volume = interp(&self.volume_kf, u).unwrap_or(self.volume).max(0.0);
        let mut fade = 1.0f64;
        if self.fade_in > 1e-6 {
            fade = fade.min((u / self.fade_in).clamp(0.0, 1.0));
        }
        if self.fade_out > 1e-6 {
            fade = fade.min(((self.len - u) / self.fade_out).clamp(0.0, 1.0));
        }
        volume * fade
    }
}

/// A keyframed value at `t`: linear between points, held before the first
/// and after the last, as `cutlass_core::project::interp` (which export uses)
/// and the preview's `interp`. None without points.
fn interp(points: &[(f64, f64)], t: f64) -> Option<f64> {
    let (first, last) = (points.first()?, points.last()?);
    if t <= first.0 {
        return Some(first.1);
    }
    if t >= last.0 {
        return Some(last.1);
    }
    points.windows(2).find(|w| t >= w[0].0 && t <= w[1].0).map(|w| {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if t1 - t0 < 1e-9 { v0 } else { v0 + (v1 - v0) * (t - t0) / (t1 - t0) }
    })
}

fn unity_gain() -> f64 {
    1.0
}

struct Shared {
    ring: Mutex<VecDeque<f32>>, // interleaved stereo
    consumed: AtomicU64,        // stereo sample-pairs played out
    decode_done: AtomicBool,
    stopped: AtomicBool,
    sample_rate: u32,
    base_t: f64,
}

#[derive(Clone)]
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

/// Reads one track's audio as a continuous interleaved-stereo stream:
/// clips play at their timeline positions, everything else is silence.
pub struct TrackReader {
    clips: Vec<AudioClip>, // sorted by start
    rate: u32,
    t: f64,
    end: f64,
    active: Option<ActiveClip>,
}

struct ActiveClip {
    idx: usize,
    dec: Option<AudioDecoder>, // None = clip has no decodable audio
    buf: std::collections::VecDeque<f32>,
    exhausted: bool,
    frac: f64, // fractional source-frame read position (for retimed clips)
}

impl TrackReader {
    pub fn new(mut clips: Vec<AudioClip>, rate: u32, from_t: f64) -> Self {
        clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
        let end = clips.iter().map(|c| c.start + c.len).fold(0.0, f64::max);
        Self { clips, rate, t: from_t, end, active: None }
    }

    pub fn finished(&self) -> bool {
        self.t >= self.end - 1e-9
    }

    /// Next `frames` stereo frames (2Ã—frames samples), advancing time.
    pub fn read(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        while out.len() < frames * 2 {
            let need_frames = frames - out.len() / 2;
            let active_idx = self
                .clips
                .iter()
                .position(|c| self.t >= c.start - 1e-9 && self.t < c.start + c.len - 1e-9);
            match active_idx {
                None => {
                    self.active = None;
                    let next_start = self
                        .clips
                        .iter()
                        .map(|c| c.start)
                        .filter(|s| *s > self.t + 1e-9)
                        .fold(f64::INFINITY, f64::min);
                    let span = if next_start.is_finite() {
                        (((next_start - self.t) * self.rate as f64).ceil() as usize).max(1)
                    } else {
                        need_frames // past the last clip: silence forever
                    };
                    let n = span.min(need_frames);
                    out.extend(std::iter::repeat(0.0f32).take(n * 2));
                    self.t += n as f64 / self.rate as f64;
                }
                Some(idx) => {
                    let clip = self.clips[idx].clone();
                    let clip_end = clip.start + clip.len;
                    let speed = clip.speed.max(0.01);
                    if self.active.as_ref().map(|a| a.idx) != Some(idx) {
                        // audio_offset slips the source read point so the audio
                        // plays earlier/later than the picture (baked-in A/V fix).
                        let src_t = (clip.src_in + (self.t - clip.start) * speed
                            - clip.audio_offset * speed)
                            .max(0.0);
                        let dec = AudioDecoder::open(&clip.path, self.rate)
                            .and_then(|mut d| {
                                d.seek(src_t)?;
                                Ok(d)
                            })
                            .ok();
                        self.active = Some(ActiveClip {
                            idx,
                            dec,
                            buf: Default::default(),
                            exhausted: false,
                            frac: 0.0,
                        });
                    }
                    let a = self.active.as_mut().unwrap();
                    let until_end = (((clip_end - self.t) * self.rate as f64).ceil() as usize).max(1);
                    let want = need_frames.min(until_end);
                    // per frame: a fade or a keyframed volume changes as it goes
                    let rate = self.rate as f64;
                    let u0 = self.t - clip.start;
                    let gain = |n: usize| clip.gain_at(u0 + n as f64 / rate) as f32;
                    // buffer enough source frames to yield `want` output frames
                    // at this speed (1 source frame per output at 1×).
                    let want_src = a.frac as usize + (want as f64 * speed).ceil() as usize + 1;
                    while a.buf.len() / 2 <= want_src && !a.exhausted {
                        match a.dec.as_mut().map(|d| d.next_chunk()) {
                            Some(Ok(Some(chunk))) => a.buf.extend(chunk),
                            _ => a.exhausted = true,
                        }
                    }
                    // resample by nearest source frame, stepping `speed` per
                    // output frame (identity when speed == 1).
                    let mut produced = 0;
                    while produced < want {
                        let sidx = a.frac as usize;
                        if sidx * 2 + 1 < a.buf.len() {
                            let g = gain(produced);
                            out.push(a.buf[sidx * 2] * g);
                            out.push(a.buf[sidx * 2 + 1] * g);
                        } else if a.exhausted {
                            out.push(0.0); // source ran short: pad silence
                            out.push(0.0);
                        } else {
                            break; // need more source; refill on the next pass
                        }
                        a.frac += speed;
                        produced += 1;
                    }
                    // drop fully-consumed source frames, keep the remainder
                    let consumed = a.frac as usize;
                    for _ in 0..(consumed * 2).min(a.buf.len()) {
                        a.buf.pop_front();
                    }
                    a.frac -= consumed as f64;
                    if produced > 0 {
                        self.t += produced as f64 / self.rate as f64;
                    } else {
                        // safety: never spin — emit silence for the request
                        out.extend(std::iter::repeat(0.0f32).take(want * 2));
                        self.t += want as f64 / self.rate as f64;
                    }
                    if self.t >= clip_end - 1e-9 {
                        self.active = None;
                    }
                }
            }
        }
        out
    }
}

/// Sums any number of track readers, clamped to [-1, 1].
pub struct MixReader {
    pub tracks: Vec<TrackReader>,
}

impl MixReader {
    pub fn read(&mut self, frames: usize) -> Vec<f32> {
        let mut acc = vec![0.0f32; frames * 2];
        for tr in self.tracks.iter_mut() {
            for (a, s) in acc.iter_mut().zip(tr.read(frames)) {
                *a = (*a + s).clamp(-1.0, 1.0);
            }
        }
        acc
    }

    pub fn finished(&self) -> bool {
        self.tracks.iter().all(|t| t.finished())
    }
}

/// Start playing `tracks` (each a clip list â€” V1, V2, â€¦) mixed together,
/// from timeline position `from_t`.
pub fn start(tracks: Vec<Vec<AudioClip>>, from_t: f64) -> anyhow::Result<PlaybackHandle> {
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

    // â”€â”€ decode thread: per-track readers â†’ mixer â†’ ring â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("cutlass-audio-decode".into())
            .spawn(move || {
                let mut mix = MixReader {
                    tracks: tracks
                        .into_iter()
                        .map(|clips| TrackReader::new(clips, sample_rate, from_t))
                        .collect(),
                };
                while !shared.stopped.load(Ordering::Relaxed) && !mix.finished() {
                    let chunk = mix.read(4800);
                    if !push_ring(&chunk, &shared) {
                        break;
                    }
                }
                shared.decode_done.store(true, Ordering::Relaxed);
            })?;
    }

    // â”€â”€ cpal thread (owns the !Send stream) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
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
        return; // contention â†’ one buffer of silence, never a blocked callback
    };
    let mut played = 0u64;
    for frame in out.chunks_mut(channels) {
        let (Some(l), Some(r)) = (ring.pop_front(), ring.pop_front()) else {
            break; // underrun â†’ silence
        };
        frame[0] = l;
        if channels > 1 {
            frame[1] = r;
        }
        played += 1;
    }
    shared.consumed.fetch_add(played, Ordering::Relaxed);
}

/// Backpressured ring push; false = playback stopped.
fn push_ring(samples: &[f32], shared: &Shared) -> bool {
    let max_ring = shared.sample_rate as usize * 2 * 2; // 2 s of stereo
    let mut offset = 0;
    while offset < samples.len() {
        if shared.stopped.load(Ordering::Relaxed) {
            return false;
        }
        // Ignore poisoning. This runs on the playback thread for every buffer;
        // if some other thread panicked while holding the ring, unwrapping here
        // would kill audio outright for the rest of the session. The ring is a
        // sample queue — the worst a panic mid-write leaves is a short glitch.
        let mut ring = shared.ring.lock().unwrap_or_else(|p| p.into_inner());
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
}
#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> AudioClip {
        AudioClip {
            path: String::new(),
            start: 0.0,
            len: 4.0,
            src_in: 0.0,
            volume: 0.8,
            speed: 1.0,
            audio_offset: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            volume_kf: Vec::new(),
        }
    }

    /// Playback's gain is export's: the volume, its keyframes (linear, held
    /// at the ends) and linear fades, in the clip's own time.
    #[test]
    fn playback_gain_follows_fades_and_volume_keyframes() {
        let c = AudioClip { fade_in: 1.0, fade_out: 2.0, ..clip() };
        let g = |u: f64| (c.gain_at(u) * 1000.0).round() / 1000.0;
        assert_eq!([g(0.0), g(0.5), g(1.5), g(3.0), g(4.0)], [0.0, 0.4, 0.8, 0.4, 0.0]);

        let c = AudioClip { volume_kf: vec![(1.0, 1.0), (2.0, 0.25)], ..clip() };
        assert_eq!([c.gain_at(0.0), c.gain_at(1.5), c.gain_at(3.0)], [1.0, 0.625, 0.25]);

        // sync offset slides the faded sound as it does in export: 0.5s later
        let c = AudioClip { fade_in: 1.0, audio_offset: 0.5, ..clip() };
        assert!((c.gain_at(1.0) - 0.4).abs() < 1e-9);
    }
}
