//! Verify audio decode + resample: the test clip carries a 440 Hz sine,
//! so decoded samples must be non-silent with sine-like RMS (~0.707 of
//! peak). `cargo run -p cutlass-engine --example audio_check -- <video>`

use cutlass_engine::audio::AudioDecoder;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: audio_check <video>");
    let mut dec = AudioDecoder::open(&path, 48_000)?;

    let mut samples: Vec<f32> = Vec::new();
    while samples.len() < 48_000 * 2 {
        match dec.next_chunk()? {
            Some(chunk) => samples.extend(chunk),
            None => break,
        }
    }
    assert!(samples.len() > 48_000, "too few samples: {}", samples.len());
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!("decoded {} samples, rms={rms:.3}, peak={peak:.3}", samples.len());
    // ffmpeg's sine source generates ~0.09 peak, not full scale
    assert!(rms > 0.02, "audio looks silent (rms={rms})");
    assert!((rms / peak - 0.707).abs() < 0.1, "not sine-like: rms/peak={}", rms / peak);

    // seek to 5s and confirm decode resumes with signal
    dec.seek(5.0)?;
    let chunk = dec.next_chunk()?.expect("chunk after seek");
    let rms2 = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
    println!("after seek(5.0): {} samples, rms={rms2:.3}", chunk.len());
    assert!(rms2 > 0.02, "silent after seek");
    println!("OK");
    Ok(())
}
