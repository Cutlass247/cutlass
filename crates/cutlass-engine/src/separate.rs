//! On-device music removal via MDX-Net vocal separation.
//!
//! Splits a clip's audio into vocals vs. everything-else and keeps the vocals,
//! so background music (often copyrighted) drops out while narration stays. The
//! model is the UVR MDX-Net "Voc_FT" ONNX net, run through pure-Rust `tract`
//! (no native runtime to bundle). The STFT/iSTFT here is a direct port of a
//! Python reference whose round-trip reconstruction error measured 1.6e-4 and
//! whose separation knocked a synthetic music bed down ~38 dB — so the maths
//! below is fixed to the model's contract; don't "tidy" the constants.

use anyhow::{Context, Result};
use realfft::RealFftPlanner;
use tract_onnx::prelude::*;

// MDX-Net Voc_FT contract. n_fft/hop set the STFT; the model consumes DIM_F of
// the N_BINS frequency bins over DIM_T frames per inference, as [1,4,DIM_F,DIM_T]
// (4 = 2 channels x {real, imag}). COMPENSATE corrects the model's global gain.
const SR: u32 = 44_100;
const N_FFT: usize = 7680;
const HOP: usize = 1024;
const DIM_F: usize = 3072;
const DIM_T: usize = 256;
const N_BINS: usize = N_FFT / 2 + 1; // 3841
const TRIM: usize = N_FFT / 2; // 3840 — discarded at each segment edge
const CHUNK: usize = HOP * (DIM_T - 1); // 261120 samples per inference window
const GEN: usize = CHUNK - 2 * TRIM; // 253440 good samples kept per window
const COMPENSATE: f32 = 1.021;

/// Periodic Hann window (matches torch.hann_window / the reference).
fn hann() -> Vec<f32> {
    (0..N_FFT)
        .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / N_FFT as f32).cos())
        .collect()
}

/// Read a WAV (expected 44.1 kHz stereo, as produced by media::decode_audio_wav)
/// de-interleaved to [L, R]. Accepts float or int samples and mono (duplicated).
fn read_wav_stereo(path: &str) -> Result<[Vec<f32>; 2]> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("open {path}"))?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            reader.samples::<f32>().collect::<Result<_, _>>()?
        }
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
    };
    let (mut l, mut r) = (Vec::new(), Vec::new());
    if ch >= 2 {
        for f in samples.chunks_exact(ch) {
            l.push(f[0]);
            r.push(f[1]);
        }
    } else {
        for s in samples {
            l.push(s);
            r.push(s);
        }
    }
    anyhow::ensure!(!l.is_empty(), "no audio samples in {path}");
    Ok([l, r])
}

struct Stft {
    fwd: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    inv: std::sync::Arc<dyn realfft::ComplexToReal<f32>>,
    win: Vec<f32>,
}

impl Stft {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        Self {
            fwd: planner.plan_fft_forward(N_FFT),
            inv: planner.plan_fft_inverse(N_FFT),
            win: hann(),
        }
    }

    /// A CHUNK-long stereo segment -> flat [4, DIM_F, DIM_T] (C-order) for the
    /// model. center=True: reflect-pad TRIM on both sides, then DIM_T hops.
    fn forward(&self, seg: &[[f32; 2]]) -> Vec<f32> {
        let mut out = vec![0.0f32; 4 * DIM_F * DIM_T];
        let mut frame = self.fwd.make_input_vec();
        let mut spec = self.fwd.make_output_vec();
        for ch in 0..2 {
            let padded = reflect_pad(seg, ch);
            for t in 0..DIM_T {
                let base = t * HOP;
                for i in 0..N_FFT {
                    frame[i] = padded[base + i] * self.win[i];
                }
                self.fwd.process(&mut frame, &mut spec).unwrap();
                let (cre, cim) = (ch * 2, ch * 2 + 1);
                for f in 0..DIM_F {
                    out[(cre * DIM_F + f) * DIM_T + t] = spec[f].re;
                    out[(cim * DIM_F + f) * DIM_T + t] = spec[f].im;
                }
            }
        }
        out
    }

    /// Model output [4, DIM_F, DIM_T] -> CHUNK-long stereo waveform (WOLA).
    fn inverse(&self, pred: &[f32]) -> Vec<[f32; 2]> {
        let full = CHUNK + 2 * TRIM;
        let mut wave = [vec![0.0f32; full], vec![0.0f32; full]];
        let mut wsum = vec![0.0f32; full];
        let mut spec = self.inv.make_input_vec();
        let mut frame = self.inv.make_output_vec();
        for ch in 0..2 {
            let (cre, cim) = (ch * 2, ch * 2 + 1);
            for t in 0..DIM_T {
                for f in 0..N_BINS {
                    spec[f] = if f < DIM_F {
                        realfft::num_complex::Complex::new(
                            pred[(cre * DIM_F + f) * DIM_T + t],
                            pred[(cim * DIM_F + f) * DIM_T + t],
                        )
                    } else {
                        realfft::num_complex::Complex::new(0.0, 0.0)
                    };
                }
                // realfft requires a purely-real DC and Nyquist bin (as irfft
                // assumes); the Nyquist bin is already zero (>= DIM_F).
                spec[0].im = 0.0;
                spec[N_BINS - 1].im = 0.0;
                // realfft's inverse is unnormalized — scale by 1/N_FFT to match
                // numpy's irfft, which the reference reconstruction was tuned to.
                self.inv.process(&mut spec, &mut frame).unwrap();
                let base = t * HOP;
                for i in 0..N_FFT {
                    let v = frame[i] / N_FFT as f32 * self.win[i];
                    wave[ch][base + i] += v;
                    if ch == 0 {
                        wsum[base + i] += self.win[i] * self.win[i];
                    }
                }
            }
        }
        // normalize the overlap-add, then trim the padded edges
        (0..CHUNK)
            .map(|i| {
                let s = wsum[TRIM + i];
                let d = if s > 1e-8 { s } else { 1.0 };
                [wave[0][TRIM + i] / d, wave[1][TRIM + i] / d]
            })
            .collect()
    }
}

/// Reflect-pad channel `ch` of a CHUNK segment by TRIM on both sides.
fn reflect_pad(seg: &[[f32; 2]], ch: usize) -> Vec<f32> {
    let n = seg.len();
    let mut out = vec![0.0f32; n + 2 * TRIM];
    for i in 0..n {
        out[TRIM + i] = seg[i][ch];
    }
    for i in 0..TRIM {
        out[TRIM - 1 - i] = seg[(i + 1).min(n - 1)][ch]; // left reflect
        out[TRIM + n + i] = seg[n.saturating_sub(2 + i)][ch]; // right reflect
    }
    out
}

/// Mix the separated vocals back over the original at `strength` and write the
/// result. strength 1.0 = vocals only (music gone), 0.0 = the original
/// untouched, in between ducks the music instead of removing it — which keeps
/// the voice full when separation isn't clean on a given recording.
///
/// Then apply makeup gain: pulling loud music out of a mix leaves whatever
/// remains much quieter than the original (a voice sitting well under the music
/// stays at its own low level), which sounds like "the audio disappeared". The
/// gain only ever boosts, targets a normal speech level, and is capped so it
/// can't clip or amplify near-silence into noise.
pub fn blend_and_write(
    raw_vocals_wav: &str,
    original_wav: &str,
    out_wav: &str,
    strength: f32,
) -> Result<()> {
    const TARGET_RMS: f32 = 0.06; // comfortable speech level
    const MAX_GAIN: f32 = 12.0;
    const PEAK_CEIL: f32 = 0.98;

    let [vl, vr] = read_wav_stereo(raw_vocals_wav)?;
    let [ol, or_] = read_wav_stereo(original_wav)?;
    let n = vl.len().min(ol.len());
    let s = strength.clamp(0.0, 1.0);
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        l.push(s * vl[i] + (1.0 - s) * ol[i]);
        r.push(s * vr[i] + (1.0 - s) * or_[i]);
    }

    let mut sq = 0.0f64;
    let mut peak = 0.0f32;
    for i in 0..n {
        sq += (l[i] * l[i] + r[i] * r[i]) as f64;
        peak = peak.max(l[i].abs()).max(r[i].abs());
    }
    let rms = if n > 0 { (sq / (2.0 * n as f64)).sqrt() as f32 } else { 0.0 };
    let mut gain = if rms > 1e-6 { (TARGET_RMS / rms).clamp(1.0, MAX_GAIN) } else { 1.0 };
    if peak * gain > PEAK_CEIL && peak > 1e-6 {
        gain = PEAK_CEIL / peak; // never clip
    }
    gain = gain.max(1.0);

    write_wav(out_wav, &l, &r, gain)
}

fn write_wav(path: &str, l: &[f32], r: &[f32], gain: f32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w =
        hound::WavWriter::create(path, spec).with_context(|| format!("create {path}"))?;
    let to_i16 = |v: f32| ((v * gain).clamp(-1.0, 1.0) * 32767.0) as i16;
    for i in 0..l.len().min(r.len()) {
        w.write_sample(to_i16(l[i]))?;
        w.write_sample(to_i16(r[i]))?;
    }
    w.finalize().context("finalize wav")?;
    Ok(())
}

/// Separate the vocals from `in_wav` (44.1 kHz stereo, from
/// media::decode_audio_wav) and write them to `out_wav` (44.1 kHz stereo,
/// 16-bit). `progress(0..=1)` is called as segments finish.
pub fn remove_music(
    in_wav: &str,
    model_path: &str,
    out_wav: &str,
    mut progress: impl FnMut(f32),
) -> Result<()> {
    let [l, r] = read_wav_stereo(in_wav)?;
    let n = l.len();
    let model = tract_onnx::onnx()
        .model_for_path(model_path)
        .with_context(|| format!("load MDX model {model_path}"))?
        .with_input_fact(
            0,
            f32::fact([1, 4, DIM_F, DIM_T]).into(),
        )?
        .into_optimized()?
        .into_runnable()?;
    let stft = Stft::new();

    let mut voc_l = vec![0.0f32; n];
    let mut voc_r = vec![0.0f32; n];
    let segments = n.div_ceil(GEN).max(1);
    for s in 0..segments {
        let start = s * GEN;
        // gather a CHUNK-long segment (zero-padded past the end)
        let mut seg = vec![[0.0f32; 2]; CHUNK];
        for i in 0..CHUNK {
            let idx = start + i;
            if idx < n {
                seg[i] = [l[idx], r[idx]];
            }
        }
        let input = stft.forward(&seg);
        let tensor = Tensor::from_shape(&[1, 4, DIM_F, DIM_T], &input)?;
        let result = model.run(tvec!(tensor.into()))?;
        let pred = result[0].as_slice::<f32>()?;
        let wav = stft.inverse(pred);
        for i in 0..GEN {
            let idx = start + i;
            if idx < n {
                voc_l[idx] = wav[i][0] * COMPENSATE;
                voc_r[idx] = wav[i][1] * COMPENSATE;
            }
        }
        progress((s + 1) as f32 / segments as f32);
    }

    // raw stem, unity gain — strength blending and makeup happen in
    // blend_and_write so changing them never re-runs the model
    write_wav(out_wav, &voc_l, &voc_r, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strength must control how much of the original is mixed back, and quiet
    /// output (what's left after loud music is pulled out) must be lifted to a
    /// usable level without ever clipping.
    #[test]
    fn blend_respects_strength_and_lifts_quiet_audio() {
        let d = std::env::temp_dir();
        let p = |n: &str| d.join(format!("cutlass_blend_{}_{}.wav", std::process::id(), n));
        let (voc, orig, out) = (p("voc"), p("orig"), p("out"));
        let (v_path, o_path, out_path) = (
            voc.to_str().unwrap().to_string(),
            orig.to_str().unwrap().to_string(),
            out.to_str().unwrap().to_string(),
        );
        let n = 44_100usize;
        // a very quiet "vocal" stem and a loud original
        let v: Vec<f32> = (0..n).map(|i| 0.014 * (i as f32 * 0.05).sin()).collect();
        let o: Vec<f32> = (0..n).map(|i| 0.30 * (i as f32 * 0.05).sin()).collect();
        write_wav(&v_path, &v, &v, 1.0).unwrap();
        write_wav(&o_path, &o, &o, 1.0).unwrap();
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |m, x| m.max(x.abs()));

        // full strength: vocals only, lifted toward a normal speech level
        blend_and_write(&v_path, &o_path, &out_path, 1.0).unwrap();
        let [l, _] = read_wav_stereo(&out_path).unwrap();
        assert!(rms(&l) > 0.03, "quiet stem should be lifted, got {}", rms(&l));
        assert!(peak(&l) <= 0.99, "must not clip, peak {}", peak(&l));

        // zero strength: the original, and already loud enough to leave alone
        blend_and_write(&v_path, &o_path, &out_path, 0.0).unwrap();
        let [l0, _] = read_wav_stereo(&out_path).unwrap();
        assert!((rms(&l0) - rms(&o)).abs() < 0.02, "should pass the original through, got {}", rms(&l0));

        // halfway sits between the two
        blend_and_write(&v_path, &o_path, &out_path, 0.5).unwrap();
        let [lh, _] = read_wav_stereo(&out_path).unwrap();
        assert!(rms(&lh) > rms(&l) * 0.5 && rms(&lh) < rms(&l0), "mid blend out of range: {}", rms(&lh));

        for f in [voc, orig, out] {
            let _ = std::fs::remove_file(f);
        }
    }
}
