//! Verify export end-to-end: two clips + a gap render to one MP4 whose
//! duration and streams check out.
//! `cargo run -p cutlass-core --example export_check -- <clipA> <clipB>`

use std::path::Path;

use cutlass_core::export::{export, has_audio, ExportSettings, Overlay, Segment};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: export_check <clipA> <clipB>");
    let b = std::env::args().nth(2).expect("usage: export_check <clipA> <clipB>");
    let a2 = a.clone(); // for the keyframe render below
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
        speed: 2.0, // 2× fast-motion (consumes 5s of source into 2.5s)
        ..Default::default()
    };
    // clip 1 → (1s cross-dissolve) → clip 2, then a gap, then clip 3.
    // The dissolve shortens clip 1's tail by 1s (2.0 → 1.0) so the total
    // stays 1.0 + 1.0 + 2.0 + 1.5 + 2.5 = 8.0.
    let segments = vec![
        Segment::Clip { path: a.clone(), src_in: 1.0, len: 1.0, fx: colored },
        Segment::Transition {
            a_path: a.clone(),
            a_src: 2.0,
            b_path: a.clone(),
            b_src: 0.0,
            dur: 1.0,
            dip: false,
        },
        Segment::Clip { path: a.clone(), src_in: 0.0, len: 2.0, fx: ClipFx::default() },
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
    let expected = 1.0 + 1.0 + 2.0 + 1.5 + 2.5;
    let out = std::env::temp_dir().join("cutlass_export_check.mp4");

    // a lower-third title over the first ~3s, and a centered one later
    use cutlass_core::export::Title;
    let titles = vec![
        Title { text: "Cutlass — it's alive".into(), start: 0.3, len: 3.0, pos_x: 0.0, pos_y: 0.3, font_size: 56.0, bg: 0.5 },
        Title { text: "Chapter 2: Effects".into(), start: 4.0, len: 3.0, pos_x: 0.0, pos_y: 0.0, font_size: 64.0, bg: 0.0 },
    ];

    let t0 = std::time::Instant::now();
    let mut last = -1.0f32;
    let encoder = export(
        &segments,
        &overlays,
        &titles,
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

    // ── keyframe render: a clip whose scale + brightness animate ───────
    use cutlass_core::export::{build_segments, ExportClip};
    use std::collections::BTreeMap;
    let mut kf = BTreeMap::new();
    kf.insert("scale".to_string(), vec![(0.0, 1.0), (3.0, 1.6)]);
    kf.insert("brightness".to_string(), vec![(0.0, -0.2), (3.0, 0.2)]);
    let kf_clip = ExportClip {
        start: 0.0,
        len: 3.0,
        src_in: 1.0,
        path: a2,
        fx: ClipFx::default(),
        trans_dur: 0.0,
        trans_dip: false,
        kf,
    };
    let kf_segs = build_segments(vec![kf_clip]);
    println!("keyframed clip -> {} sub-segments", kf_segs.len());
    let kf_out = std::env::temp_dir().join("cutlass_export_kf.mp4");
    export(&kf_segs, &[], &[], &kf_out, &ExportSettings { width: 1280, height: 720, fps: 30 }, &mut |_| {})?;
    let kf_dur = probe_duration_s(&kf_out)?;
    println!("keyframed export duration {kf_dur:.2}s (expected 3.00)");
    assert!((kf_dur - 3.0).abs() < 0.3, "keyframe duration {kf_dur:.2} != 3.0");

    println!("OK");
    Ok(())
}
