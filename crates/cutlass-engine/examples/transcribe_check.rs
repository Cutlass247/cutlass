//! Verify on-device transcription against the TTS test clip, which speaks
//! a known sentence. `cargo run -p cutlass-engine --example
//! transcribe_check -- <video> <model.bin>`

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: transcribe_check <video> <model>");
    let model = std::env::args().nth(2).expect("usage: transcribe_check <video> <model>");

    let t0 = Instant::now();
    let words = cutlass_engine::transcribe::transcribe(&path, &model)?;
    let secs = t0.elapsed().as_secs_f64();

    let joined = words
        .iter()
        .map(|w| w.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    println!("{} words in {secs:.1}s: {joined}", words.len());
    for w in words.iter().take(8) {
        println!("  {:>6.2}-{:>5.2}s  {}", w.start, w.end, w.text);
    }

    for expected in ["quick", "fox", "cutlass", "video", "delete"] {
        assert!(
            joined.contains(expected),
            "expected word '{expected}' missing from transcript"
        );
    }
    let monotonic = words.windows(2).all(|p| p[1].start >= p[0].start - 0.01);
    assert!(monotonic, "word timestamps not monotonic");
    println!("OK");
    Ok(())
}
