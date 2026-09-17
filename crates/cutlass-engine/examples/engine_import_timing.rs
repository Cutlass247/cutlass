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

/// The same thumbnail work, split across threads.
///
/// Each worker opens its own MediaEngine and takes every Nth timestamp. The
/// seeks are independent — nothing shares state — so the only question is
/// whether decoding keyframes scales with cores or saturates something else.
///
/// Note this is NOT the concurrency that was tried and reverted. That one ran
/// several FILES at once, which made the first clip appear later; this splits
/// ONE file, so the single clip the person is waiting for appears sooner.
fn thumbs_parallel(path: &str, frames_target: f64, workers: usize) -> anyhow::Result<(u32, f64)> {
    let t = Instant::now();
    let duration_s = MediaEngine::open(path)?.duration_s();
    let scrub_fps = (frames_target / duration_s.max(0.1)).min(10.0);
    let interval = 1.0 / scrub_fps;
    let width = cutlass_core::media::SCRUB_WIDTH;
    let n = (duration_s * scrub_fps).ceil() as u32;

    let made = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    std::thread::scope(|s| {
        for w in 0..workers {
            let path = path.to_string();
            let made = made.clone();
            s.spawn(move || {
                let Ok(mut eng) = MediaEngine::open(&path) else { return };
                let mut i = w as u32 + 1;
                while i <= n {
                    if eng.keyframe_at((i - 1) as f64 * interval, width).is_ok() {
                        made.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    i += workers as u32;
                }
            });
        }
    });
    Ok((made.load(std::sync::atomic::Ordering::Relaxed), t.elapsed().as_secs_f64()))
}

/// The OTHER half of import, and the half this harness used to ignore.
///
/// `import_with_engine` decodes the entire audio track to build the scrub
/// waveform for anything up to 30 minutes. That is linear in clip length,
/// where the thumbnails are closer to square-root — so on a long recording it
/// can be the larger cost by far, and timing only the thumbnails says import
/// is fast while the user watches it not be.
fn waveform_for(path: &str) -> (usize, f64) {
    let t = Instant::now();
    let peaks = cutlass_engine::audio::waveform_peaks(path, 1200);
    (peaks.len(), t.elapsed().as_secs_f64())
}

fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: engine_import_timing <clip> [clip...]");
        std::process::exit(2);
    }

    println!("cores: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    println!();

    // Both halves, and the total the person actually waits for.
    println!("one file, decoding in-process:");
    println!(
        "  {:<22} {:>9} {:>10} {:>10} {:>10} {:>9}",
        "file", "duration", "thumbs", "waveform", "TOTAL", "x realtime"
    );
    for f in &files {
        // How long until we know enough to SHOW the clip? Everything the bin
        // needs — duration and dimensions — is metadata. If this is fast and
        // the thumbnails are slow, the thumbnails do not belong in the wait.
        let t_probe = Instant::now();
        let probe = MediaEngine::open(f).map(|e| (e.duration_s(), e.dimensions()));
        let probe_s = t_probe.elapsed().as_secs_f64();
        println!(
            "  [probe only: {:.3}s  -> {:?}]",
            probe_s,
            probe.as_ref().map(|(d, wh)| (*d as u32, *wh)).unwrap_or_default()
        );
        let dur = probe.map(|(d, _)| d).unwrap_or(0.0);
        let (_n, t_thumbs) = thumbs_for(f, 240.0).unwrap_or((0, 0.0));
        let (_p, t_wave) = waveform_for(f);
        let total = t_thumbs + t_wave;
        println!(
            "  {:<22} {:>8.0}s {:>9.2}s {:>9.2}s {:>9.2}s {:>8.0}x",
            std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy(),
            dur,
            t_thumbs,
            t_wave,
            total,
            if total > 0.0 { dur / total } else { 0.0 }
        );
    }

    // Does splitting ONE file's thumbnail seeks across cores actually help?
    println!();
    println!("thumbnails for ONE file, split across workers:");
    println!("  {:<22} {:>8} {:>8} {:>8} {:>8} {:>8}", "file", "1", "2", "4", "6", "8");
    for f in &files {
        let mut row = String::new();
        let mut base = 0.0f64;
        for w in [1usize, 2, 4, 6, 8] {
            let (_n, t) = thumbs_parallel(f, 240.0, w).unwrap_or((0, 0.0));
            if w == 1 {
                base = t;
            }
            row.push_str(&format!(" {:>7.2}s", t));
        }
        println!(
            "  {:<22}{}   (1 worker = {:.2}s)",
            std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy(),
            row,
            base
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
