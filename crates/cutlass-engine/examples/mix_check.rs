//! Headless proof of multi-track audio routing: track 1 owns t∈[0,1),
//! track 2 owns t∈[1,2). Both must be audible in the mixed stream, and
//! the mixer must go silent after both end. (Deliberately phase-immune —
//! codec seeks aren't sample-accurate, so summing two coherent sines is
//! not a stable assertion.)
//! `cargo run -p cutlass-engine --example mix_check -- <video-with-audio>`

use cutlass_engine::player::{AudioClip, MixReader, TrackReader};

fn rms(s: &[f32]) -> f32 {
    (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: mix_check <video>");
    let rate = 48_000u32;
    let t1 = vec![AudioClip { path: path.clone(), start: 0.0, len: 1.0, src_in: 0.0, volume: 1.0 }];
    let t2 = vec![AudioClip { path, start: 1.0, len: 1.0, src_in: 5.0, volume: 1.0 }];
    let mut mix = MixReader {
        tracks: vec![TrackReader::new(t1, rate, 0.0), TrackReader::new(t2, rate, 0.0)],
    };

    let from_t1 = mix.read(rate as usize); // only track 1 active
    let from_t2 = mix.read(rate as usize); // only track 2 active
    let tail = mix.read((rate / 2) as usize); // past both: silence
    let (r1, r2, rt) = (rms(&from_t1), rms(&from_t2), rms(&tail));
    println!("track1 region rms={r1:.3}, track2 region rms={r2:.3}, tail rms={rt:.4}");
    assert!(r1 > 0.02, "track 1 inaudible through the mixer");
    assert!(r2 > 0.02, "track 2 inaudible through the mixer");
    assert!(rt < 0.001, "mixer not silent after all clips ended");
    assert!(mix.finished(), "mixer should report finished");
    println!("OK");
    Ok(())
}
