//! Verify the engine-native import primitives: one-pass frame sampling
//! and in-process waveform peaks.
//! `cargo run -p cutlass-engine --example proxy_check -- <video>`

use cutlass_engine::MediaEngine;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: proxy_check <video>");

    let t0 = std::time::Instant::now();
    let mut eng = MediaEngine::open(&path)?;
    let dur = eng.duration_s();
    let mut sizes = Vec::new();
    let n = eng.sample_frames(1.0, 480, |f| {
        anyhow::ensure!(f.width <= 480 && f.data.len() as u32 == f.width * f.height * 4);
        sizes.push(f.data.len());
        Ok(())
    })?;
    let expect = dur.floor() as u32 + 1;
    println!("sampled {n} frames (expected ~{expect}) from {dur:.1}s in {:.2}s", t0.elapsed().as_secs_f64());
    assert!(n >= expect.saturating_sub(1) && n <= expect + 1, "bad sample count");

    let wf = cutlass_engine::audio::waveform_peaks(&path, 1200);
    let nz = wf.iter().filter(|p| **p > 0.05).count();
    println!("waveform: {} buckets, {} above 5%", wf.len(), nz);
    assert!(wf.len() > 300, "waveform too short");
    assert!(nz > wf.len() / 4, "waveform mostly silent for a tone clip");
    println!("OK");
    Ok(())
}
