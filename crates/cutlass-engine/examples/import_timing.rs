//! Where import time actually goes, stage by stage.
//!
//! `cargo run -p cutlass-core --example import_timing -- <clip> [clip...]`
//!
//! Import felt slow and the obvious suspects are all plausible — the VFR
//! conform re-encodes the whole file, thumbnails JPEG-encode up to 480 frames,
//! and the waveform spawns a second ffmpeg to decode every sample. Guessing
//! which dominates is how you optimise the wrong one.

use std::time::Instant;

fn secs(t: Instant) -> f64 {
    t.elapsed().as_secs_f64()
}

fn main() -> anyhow::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: import_timing <clip> [clip...]");
        std::process::exit(2);
    }
    cutlass_core::media::ensure_ffmpeg()?;

    println!(
        "{:<26} {:>7} {:>8} {:>9} {:>9} {:>9} {:>9}",
        "file", "dur", "vfr?", "probe", "thumbs", "waveform", "TOTAL"
    );

    for f in &files {
        let path = std::path::Path::new(f);
        if !path.exists() {
            println!("{:<26} (missing)", f);
            continue;
        }
        // Start from cold: the thumbnail cache makes a second import nearly
        // free, which would hide the cost entirely.
        if let Ok(dir) = cutlass_core::media::cache_dir(path) {
            let _ = std::fs::remove_dir_all(&dir);
        }

        let all = Instant::now();

        let t = Instant::now();
        let dur = cutlass_core::media::probe_duration_s(path).unwrap_or(0.0);
        let (_w, _h) = cutlass_core::media::probe_dimensions(path);
        let probe = secs(t);

        // Is this one of the files that triggers a full re-encode?
        let vfr = cutlass_engine::MediaEngine::open(&path.to_string_lossy())
            .map(|e| e.is_vfr())
            .unwrap_or(false);

        let t = Instant::now();
        let _ = cutlass_core::media::scrub_proxy(path, dur);
        let thumbs = secs(t);

        let t = Instant::now();
        let _ = cutlass_core::media::waveform(path);
        let wave = secs(t);

        println!(
            "{:<26} {:>6.1}s {:>8} {:>8.2}s {:>8.2}s {:>8.2}s {:>8.2}s",
            path.file_name().unwrap_or_default().to_string_lossy(),
            dur,
            if vfr { "VFR" } else { "cfr" },
            probe,
            thumbs,
            wave,
            secs(all)
        );
    }

    println!();
    println!("A VFR file additionally re-encodes end to end before any of this,");
    println!("which is not measured here — run it through the app to see that cost.");
    Ok(())
}
