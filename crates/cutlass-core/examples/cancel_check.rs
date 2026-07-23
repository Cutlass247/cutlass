//! Verify a running export can be cancelled: kick off a slow render, flip
//! the cancel flag mid-way, and confirm it errors with "cancelled" and the
//! partial output file is removed.
//! `cargo run -p cutlass-core --example cancel_check -- <clip>`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: cancel_check <clip>");
    cutlass_core::media::ensure_ffmpeg()?;
    // 4K makes the encode slow enough to cancel mid-flight
    let segs = vec![Segment::Clip { path: a, src_in: 0.0, len: 8.0, fx: ClipFx::default(), lut: String::new() }];
    let out = std::env::temp_dir().join("cutlass_cancel_check.mp4");

    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        c2.store(true, Ordering::Relaxed);
    });

    let settings = ExportSettings { width: 3840, height: 2160, fps: 30, ..Default::default() };
    let res = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &cancel);

    match &res {
        Ok(_) => anyhow::bail!("export finished instead of cancelling"),
        Err(e) => {
            let msg = format!("{e}");
            println!("cancelled with: {msg}");
            assert!(msg.contains("cancel"), "unexpected error: {msg}");
        }
    }
    assert!(!out.exists(), "partial output was not removed");
    println!("OK cancelled + partial removed");
    Ok(())
}
