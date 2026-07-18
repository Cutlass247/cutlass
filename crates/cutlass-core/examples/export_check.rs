//! Verify export end-to-end: two clips + a gap render to one MP4 whose
//! duration and streams check out.
//! `cargo run -p cutlass-core --example export_check -- <clipA> <clipB>`

use std::path::Path;

use cutlass_core::export::{export, has_audio, ExportSettings, Overlay, Segment};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: export_check <clipA> <clipB>");
    let b = std::env::args().nth(2).expect("usage: export_check <clipA> <clipB>");
    cutlass_core::media::ensure_ffmpeg()?;

    use cutlass_core::export::ClipFx;
    // clip 1: color + fades + gain; clip 2: transform (scale/pos/rotate)
    let colored = ClipFx {
        brightness: 0.1,
        saturation: 1.4,
        fade_in: 0.5,
        fade_out: 0.5,
        volume: 0.6,
        ..Default::default()
    };
    let transformed = ClipFx {
        scale: 0.7,
        pos_x: 0.1,
        rot: 8.0,
        ..Default::default()
    };
    let segments = vec![
        Segment::Clip { path: a.clone(), src_in: 1.0, len: 2.0, fx: colored },
        Segment::Gap { len: 1.5 },
        Segment::Clip { path: b, src_in: 0.5, len: 2.5, fx: transformed },
    ];
    // V2 overlay spanning the gap — must not change output duration
    let overlays = vec![Overlay {
        path: a,
        src_in: 4.0,
        len: 2.0,
        start: 2.5,
        fx: ClipFx { contrast: 1.2, ..Default::default() },
    }];
    let expected = 2.0 + 1.5 + 2.5;
    let out = std::env::temp_dir().join("cutlass_export_check.mp4");

    let t0 = std::time::Instant::now();
    let mut last = -1.0f32;
    let encoder = export(
        &segments,
        &overlays,
        &out,
        &ExportSettings { width: 1280, height: 720, fps: 30 },
        &mut |p| {
            if p - last >= 0.25 {
                println!("  progress {:.0}%", p * 100.0);
                last = p;
            }
        },
    )?;
    let took = t0.elapsed().as_secs_f64();

    let dur = probe_duration_s(&out)?;
    let size_kb = std::fs::metadata(&out)?.len() / 1024;
    println!(
        "exported {:.2}s in {took:.1}s via {encoder} -> {} ({size_kb} KB, duration {dur:.2}s)",
        expected,
        out.display()
    );
    assert!((dur - expected).abs() < 0.3, "duration {dur:.2} != expected {expected:.2}");
    assert!(has_audio(&out.to_string_lossy()), "exported file has no audio stream");
    assert!(size_kb > 50, "suspiciously small output");
    println!("OK");
    Ok(())
}
