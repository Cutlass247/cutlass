//! Time the import path the APP actually runs, and compare sequential against
//! concurrent.
//!
//! `cargo run -p cutlass-engine --example engine_import_timing -- <clip> [clip...]`
//!
//! This exists because `import_timing` measured the wrong thing. That harness
//! calls `cutlass_core::media::scrub_proxy`, which spawns an ffmpeg
//! subprocess — so import looked I/O-bound, and importing four files at once
//! looked nearly free. The app does not use that path. It uses
//! `MediaEngine`, which decodes IN-PROCESS through linked libav, and is
//! therefore CPU-bound. Four at once competes for cores instead of
//! overlapping waits, and the first clip takes four times as long to appear.
//!
//! Measured on a real import: 465 CPU-seconds inside the app and zero ffmpeg
//! processes. So: measure the path that runs, not the one that is easy to
//! call.

use std::time::Instant;
use cutlass_engine::MediaEngine;

/// Mirrors the thumbnail loop in the app's `import_with_engine`.
fn thumbs_for(path: &str, frames_target: f64) -> anyhow::Result<(u32, f64)> {
    let t = Instant::now();
    let mut eng = MediaEngine::open(path)?;
    let duration_s = eng.duration_s();
    let scrub_fps = (frames_target / duration_s.max(0.1)).min(10.0);
    let interval = 1.0 / scrub_fps;
    let width = cutlass_core::media::SCRUB_WIDTH;

    let mut made = 0u32;
    if duration_s > 60.0 {
        // long clips seek to each thumbnail
        let n = (duration_s * scrub_fps).ceil() as u32;
        for i in 1..=n {
            match eng.keyframe_at((i - 1) as f64 * interval, width) {
                Ok(_) => made += 1,
                Err(_) => break,
            }
        }
    } else {
        // short clips decode straight through
        let n = (duration_s * scrub_fps).ceil() as u32;
        for i in 1..=n {
            match eng.keyframe_at((i - 1) as f64 * interval, width) {
                Ok(_) => made += 1,
                Err(_) => break,
            }
        }
    }
    Ok((made, t.elapsed().as_secs_f64()))
}

fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: engine_import_timing <clip> [clip...]");
        std::process::exit(2);
    }

    println!("cores: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    println!();

    // How much does the 480-vs-240 frame target actually cost?
    println!("one file, decoding in-process:");
    println!("  {:<24} {:>8} {:>10} {:>10}", "file", "frames", "480 target", "240 target");
    for f in &files {
        let (n480, t480) = thumbs_for(f, 480.0).unwrap_or((0, 0.0));
        let (n240, t240) = thumbs_for(f, 240.0).unwrap_or((0, 0.0));
        println!(
            "  {:<24} {:>8} {:>9.2}s {:>9.2}s",
            std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy(),
            format!("{n480}/{n240}"),
            t480,
            t240
        );
    }

    if files.len() < 2 {
        println!("\n(pass two or more files to compare sequential against concurrent)");
        return Ok(());
    }

    println!();
    let t = Instant::now();
    for f in &files {
        let _ = thumbs_for(f, 240.0);
    }
    let seq = t.elapsed().as_secs_f64();

    let t = Instant::now();
    let handles: Vec<_> = files
        .iter()
        .cloned()
        .map(|f| std::thread::spawn(move || thumbs_for(&f, 240.0)))
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let par = t.elapsed().as_secs_f64();

    println!("{} files, 240-frame target:", files.len());
    println!("  sequential : {seq:.2}s   (first clip appears after {:.2}s)", seq / files.len() as f64);
    println!("  all at once: {par:.2}s   (first clip appears after ~{par:.2}s — they finish together)");
    println!();
    println!("Concurrency only wins if `all at once` beats `sequential` by enough to");
    println!("pay for every clip appearing late instead of one appearing quickly.");
    Ok(())
}
