//! Harness: a caption too wide for the frame must wrap, not run off the edges.
//! `cargo run -p cutlass-core --example title_fit_check -- <clip> <outdir>`
//!
//! drawtext never wraps. A six-word caption in a vertical (1080×1920) export
//! is drawn at 82px, and "honestly everything completely changed after that"
//! came out as "verything completely changed" — both ends cut off. This
//! renders the same second of footage with and without the caption, in every
//! clip shape, and finds the caption by where the two frames differ.

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment, Title};
use std::path::Path;

const CAPTION: &str = "honestly everything completely changed after that";

fn frame_gray(video: &Path, w: u32, h: u32) -> anyhow::Result<Vec<u8>> {
    let ffmpeg = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());
    let out = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-ss", "0.5", "-i"])
        .arg(video)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()?;
    anyhow::ensure!(out.stdout.len() == (w * h) as usize, "frame grab gave {} bytes", out.stdout.len());
    Ok(out.stdout)
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let src = a.next().expect("usage: title_fit_check <clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let segs = vec![Segment::Clip {
        path: src,
        src_in: 0.0,
        len: 1.0,
        fx: ClipFx::default(),
        lut: String::new(),
    }];
    // what Create's captions use: lower third, 46px at 1080p, dark band
    let caption = Title {
        text: CAPTION.into(),
        start: 0.0,
        len: 1.0,
        pos_x: 0.0,
        pos_y: 0.34,
        font_size: 46.0,
        bg: 0.55,
    };

    for (name, w, h) in [("vertical", 1080u32, 1920u32), ("square", 1080, 1080), ("wide", 1920, 1080)] {
        let s = ExportSettings { width: w, height: h, fps: 30, ..Default::default() };
        let with = dir.join(format!("{name}-caption.mp4"));
        let without = dir.join(format!("{name}-plain.mp4"));
        export(&segs, &[], std::slice::from_ref(&caption), &with, &s, &mut |_| {}, &cancel)?;
        export(&segs, &[], &[], &without, &s, &mut |_| {}, &cancel)?;
        let (a, b) = (frame_gray(&with, w, h)?, frame_gray(&without, w, h)?);
        // columns and rows the caption changed (beyond encoder noise)
        let mut xs = (u32::MAX, 0u32);
        let mut ys = (u32::MAX, 0u32);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if (a[i] as i32 - b[i] as i32).abs() > 40 {
                    xs = (xs.0.min(x), xs.1.max(x));
                    ys = (ys.0.min(y), ys.1.max(y));
                }
            }
        }
        if xs.0 == u32::MAX {
            println!("  FAIL {name}: no caption found in the frame");
        } else if xs.0 <= 1 || xs.1 >= w - 2 {
            println!("  FAIL {name}: caption spans x={}..{} of {w} — cut off at the edges", xs.0, xs.1);
        } else {
            println!(
                "  PASS {name} {w}x{h}: caption x={}..{} y={}..{} ({} px clear each side)",
                xs.0,
                xs.1,
                ys.0,
                ys.1,
                xs.0.min(w - 1 - xs.1)
            );
        }
    }
    Ok(())
}
