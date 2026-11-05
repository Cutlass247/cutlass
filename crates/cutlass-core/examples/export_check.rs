//! Verify export end-to-end: two clips + a gap render to one MP4 whose
//! duration and streams check out.
//! `cargo run -p cutlass-core --example export_check -- <clipA> <clipB>`


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
        Segment::Clip { path: a.clone(), src_in: 1.0, len: 1.0, fx: colored, lut: String::new() },
        Segment::Transition {
            a_path: a.clone(),
            a_src: 2.0,
            b_path: a.clone(),
            b_src: 0.0,
            dur: 1.0,
            dip: false,
            // the outgoing side keeps its look through the dissolve
            a_fx: ClipFx { saturation: 1.4, scale: 0.8, ..Default::default() },
            a_lut: String::new(),
            b_fx: ClipFx::default(),
            b_lut: String::new(),
        },
        Segment::Clip { path: a.clone(), src_in: 0.0, len: 2.0, fx: ClipFx::default(), lut: String::new() },
        Segment::Gap { len: 1.5 },
        Segment::Clip { path: b, src_in: 0.5, len: 2.5, fx: transformed, lut: String::new() },
    ];
    // V2 overlay spanning the gap — must not change output duration
    let overlays = vec![Overlay {
        path: a,
        src_in: 4.0,
        len: 2.0,
        start: 2.5,
        fx: ClipFx { contrast: 1.2, ..Default::default() },
        audio_only: false,
        kf: Default::default(),
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
        &ExportSettings { width: 1280, height: 720, fps: 30, ..Default::default() },
        &mut |p| {
            if p - last >= 0.25 {
                println!("  progress {:.0}%", p * 100.0);
                last = p;
            }
        },
        &std::sync::atomic::AtomicBool::new(false),
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
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
        kf,
    };
    let kf_segs = build_segments(vec![kf_clip]);
    println!("keyframed clip -> {} sub-segments", kf_segs.len());
    let kf_out = std::env::temp_dir().join("cutlass_export_kf.mp4");
    export(&kf_segs, &[], &[], &kf_out, &ExportSettings { width: 1280, height: 720, fps: 30, ..Default::default() }, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
    let kf_dur = probe_duration_s(&kf_out)?;
    println!("keyframed export duration {kf_dur:.2}s (expected 3.00)");
    assert!((kf_dur - 3.0).abs() < 0.3, "keyframe duration {kf_dur:.2} != 3.0");

    dissolve_keeps_speed()?;

    println!("OK");
    Ok(())
}

/// A dissolve between retimed clips plays each at its own speed, from where
/// the clip itself is. The windows used to be read at normal speed from
/// positions that ignored speed: out of a 2x clip, the picture jumped back
/// three seconds of footage and slowed to 1x for the dissolve.
///
/// The source says in every frame which of its seconds is showing: the
/// picture's brightness is 20 + 6s, the tone is 200 + 50s Hz.
fn dissolve_keeps_speed() -> anyhow::Result<()> {
    use cutlass_core::export::{build_segments, ClipFx, ExportClip};
    let ff = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());
    let dir = std::env::temp_dir();
    let ramp = dir.join("cutlass_export_ramp.mkv");
    let made = std::process::Command::new(&ff)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=black:s=320x180:r=30:d=24,format=yuv420p,geq=lum='20+6*T':cb=128:cr=128")
        .args(["-f", "lavfi", "-i", "aevalsrc='0.5*sin(2*PI*(200*t+25*t*t))':s=48000:d=24"])
        .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(&ramp)
        .status()?;
    anyhow::ensure!(made.success(), "could not make the ramp source");

    // A: 4s at 2x from source 2s (2..10); B: 3s at half speed from 15s, with
    // a 1s dissolve into it, so its head is source 14.5..15.
    let clip = |start: f64, len: f64, src_in: f64, speed: f64, trans: f64| ExportClip {
        start,
        len,
        src_in,
        path: ramp.to_string_lossy().into_owned(),
        fx: ClipFx { speed, ..Default::default() },
        lut: String::new(),
        trans_dur: trans,
        trans_dip: false,
        kf: Default::default(),
    };
    let segs = build_segments(vec![clip(0.0, 4.0, 2.0, 2.0, 0.0), clip(4.0, 3.0, 15.0, 0.5, 1.0)]);
    // ProRes, every frame its own: H.264 holds a flat picture that changes by
    // a level or two, which reads as the footage lagging
    let out = dir.join("cutlass_export_dissolve_speed.mov");
    let settings = ExportSettings {
        width: 640,
        height: 360,
        fps: 30,
        format: cutlass_core::export::ExportFormat::MovProres,
        ..Default::default()
    };
    export(&segs, &[], &[], &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;

    // which source second each side shows at timeline t
    let a_src = |t: f64| 2.0 + 2.0 * t;
    let b_src = |t: f64| 15.0 + 0.5 * (t - 4.0);
    let luma = |s: f64| (20.0 + 6.0 * s).floor(); // geq makes whole levels
    let expected_y = |t: f64| {
        if t < 3.0 {
            luma(a_src(t))
        } else if t < 4.0 {
            let p = t - 3.0; // the dissolve's progress
            (1.0 - p) * luma(a_src(t)) + p * luma(b_src(t))
        } else {
            luma(b_src(t))
        }
    };

    // the picture: mean brightness of every frame
    let pix = std::process::Command::new(&ff)
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vf", "extractplanes=y,scale=40:22:flags=area", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()?
        .stdout;
    let frames: Vec<f64> = pix
        .chunks_exact(40 * 22)
        .map(|f| f.iter().map(|&v| v as f64).sum::<f64>() / f.len() as f64)
        .collect();
    let mut worst = (0.0f64, 0.0f64);
    for (n, y) in frames.iter().enumerate() {
        let t = n as f64 / 30.0;
        let off = (y - expected_y(t)).abs();
        if off > worst.0 {
            worst = (off, t);
        }
    }
    let pic_ok = frames.len() >= 205 && worst.0 < 3.0;
    println!(
        "{} dissolve picture at clip speed: {} frames, furthest {:.1} levels from expected at {:.2}s",
        if pic_ok { "PASS" } else { "FAIL" },
        frames.len(),
        worst.0,
        worst.1
    );

    // the sound: pitch from zero crossings over 0.1s, in each clip's body
    // and at the start of the dissolve, where A is still 90% of the mix
    let pcm = std::process::Command::new(&ff)
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"])
        .output()?
        .stdout;
    let samples: Vec<i16> = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let pitch_at = |t: f64| {
        let from = (t * 48000.0) as usize;
        let win = &samples[from.min(samples.len())..(from + 4800).min(samples.len())];
        let crossings = win.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        crossings as f64 / 2.0 / 0.1
    };
    let tone = |s: f64| 200.0 + 50.0 * s;
    let mut sound_ok = true;
    for (t, want, what) in [
        (1.0, tone(a_src(1.05)), "A's body"),
        (3.0, tone(a_src(3.05)), "the dissolve's start"),
        (5.0, tone(b_src(5.05)), "B's body"),
    ] {
        let got = pitch_at(t);
        let ok = (got - want).abs() < 25.0;
        sound_ok &= ok;
        println!(
            "{} dissolve sound at clip speed, {what}: {got:.0} Hz, expected {want:.0}",
            if ok { "PASS" } else { "FAIL" }
        );
    }
    anyhow::ensure!(pic_ok && sound_ok, "a dissolve between retimed clips lost their speed");
    Ok(())
}
