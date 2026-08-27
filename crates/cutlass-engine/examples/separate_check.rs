//! Ad-hoc check: run the Rust MDX separation on an audio file.
//! Usage: cargo run --example separate_check -- <in.wav> <model.onnx> <out.wav>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: separate_check <in.wav> <model.onnx> <out.wav>");
        std::process::exit(2);
    }
    let t = std::time::Instant::now();
    cutlass_engine::separate::remove_music(&a[1], &a[2], &a[3], |p| {
        eprint!("\rseparating {:.0}%", p * 100.0);
    })?;
    eprintln!("\rdone in {:.1}s -> {}", t.elapsed().as_secs_f64(), a[3]);
    Ok(())
}
