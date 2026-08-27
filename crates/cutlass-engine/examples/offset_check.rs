//! Reproduce the audio-offset preview behaviour on a single clip.
//! Usage: cargo run --example offset_check -- <media> <len_s> <audio_offset_s>
use cutlass_engine::player::{AudioClip, TrackReader};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let path = a[1].clone();
    let len: f64 = a[2].parse().unwrap();
    let off: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(0.0);
    let rate = 48_000u32;
    let clip = AudioClip {
        path,
        start: 0.0,
        len,
        src_in: 0.0,
        volume: 1.0,
        speed: 1.0,
        audio_offset: off,
    };
    let mut tr = TrackReader::new(vec![clip], rate, 0.0);
    let mut total = 0usize; // stereo frames emitted
    let mut last_nonsilent = 0usize;
    let mut first_silence_run: Option<usize> = None;
    let mut silence_run = 0usize;
    while !tr.finished() && total < (len as usize + 5) * rate as usize {
        let chunk = tr.read(4096);
        if chunk.is_empty() {
            break;
        }
        for (i, f) in chunk.chunks(2).enumerate() {
            let loud = f[0].abs() > 1e-3 || f.get(1).map(|x| x.abs() > 1e-3).unwrap_or(false);
            if loud {
                last_nonsilent = total + i;
                silence_run = 0;
            } else {
                silence_run += 1;
                // remember the start of the long trailing-silence region
                if silence_run > rate as usize / 2 && first_silence_run.is_none() {
                    first_silence_run = Some(total + i - silence_run);
                }
            }
        }
        total += chunk.len() / 2;
    }
    let s = |f: usize| f as f64 / rate as f64;
    println!("offset={off:+.2}s  clip_len={len:.1}s");
    println!("  emitted {:.2}s of timeline audio", s(total));
    println!("  last non-silent sample at {:.2}s", s(last_nonsilent));
    println!("  audio present for {:.2}s of the {:.1}s clip", s(last_nonsilent), len);
    Ok(())
}
