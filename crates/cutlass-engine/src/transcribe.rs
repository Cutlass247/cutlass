//! On-device speech-to-text via whisper.cpp. Nothing leaves the machine —
//! that's a product promise, not an implementation detail.

use anyhow::Context as _;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::AudioDecoder;

pub const WHISPER_RATE: u32 = 16_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Word {
    pub text: String,
    /// source-media time, seconds
    pub start: f64,
    pub end: f64,
}

/// Decode the file's audio to 16 kHz mono and transcribe with word-level
/// timestamps. `model_path` is a ggml whisper model.
pub fn transcribe(path: &str, model_path: &str) -> anyhow::Result<Vec<Word>> {
    // 1. audio → 16 kHz mono f32
    let mut dec = AudioDecoder::open_mono(path, WHISPER_RATE)?;
    let mut samples: Vec<f32> = Vec::new();
    while let Some(chunk) = dec.next_chunk()? {
        samples.extend(chunk);
    }
    if samples.len() < WHISPER_RATE as usize / 2 {
        return Ok(Vec::new()); // under half a second of audio
    }

    // 2. whisper full pass, one word per segment
    let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
        .with_context(|| format!("load whisper model {model_path}"))?;
    let mut state = ctx.create_state().context("whisper state")?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // whisper.cpp defaults to 4 threads; use (almost) all cores so a long
    // clip transcribes far faster, leaving one core for the UI/playback.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(4);
    params.set_n_threads(threads as std::os::raw::c_int);
    params.set_language(Some("en"));
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_max_len(1);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state.full(params, &samples).context("whisper full")?;

    // 3. segments (≈ words) → Word list; timestamps are centiseconds
    let n = state.full_n_segments();
    let mut words = Vec::with_capacity(n as usize);
    for i in 0..n {
        let Some(seg) = state.get_segment(i) else { continue };
        let text = seg.to_str_lossy().context("segment text")?.trim().to_string();
        if text.is_empty() || text.starts_with('[') || text.starts_with('(') {
            continue; // [BLANK_AUDIO], (breathing), etc.
        }
        words.push(Word {
            text,
            start: seg.start_timestamp() as f64 / 100.0,
            end: seg.end_timestamp() as f64 / 100.0,
        });
    }
    Ok(words)
}
