//! Audit harness: a cancelled staged export must leave nothing behind.
//! `cargo run -p cutlass-core --example cancel_staged -- <clip> <outdir>`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let src = a.next().expect("usage: cancel_staged <clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    // 40 segments -> staged into 5 parts, long enough to cancel mid-flight
    let segs: Vec<Segment> = (0..40)
        .map(|k| Segment::Clip {
            path: src.clone(),
            src_in: (k as f64 * 0.5) % 20.0,
            len: 1.0,
            fx: ClipFx::default(),
            lut: String::new(),
        })
        .collect();

    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        c2.store(true, Ordering::Relaxed);
    });

    let out = dir.join("cancelled.mp4");
    let settings = ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() };
    let res = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &cancel);
    println!("export returned: {:?}", res.as_ref().err().map(|e| e.to_string()));

    // what did it leave on disk?
    let mut leftovers = Vec::new();
    for e in std::fs::read_dir(&dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(".cutlass_export_") {
            let n = std::fs::read_dir(e.path()).map(|d| d.count()).unwrap_or(0);
            let bytes: u64 = std::fs::read_dir(e.path())
                .map(|d| d.filter_map(|f| f.ok()).filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum())
                .unwrap_or(0);
            leftovers.push(format!("{name} ({n} files, {} KB)", bytes / 1024));
        }
    }
    if leftovers.is_empty() {
        println!("PASS: no part folders left behind");
    } else {
        println!("FAIL: stranded {} folder(s): {:?}", leftovers.len(), leftovers);
    }
    println!(
        "partial output present: {}",
        out.exists()
    );
    Ok(())
}
