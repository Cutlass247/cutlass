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
        lut: String::new(),
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

    let probe = Probe::new()?;
    let dissolve = dissolve_keeps_speed(&probe)?;
    let overlays = overlays_keep_their_settings(&probe)?;
    let finish = boxes_and_finish_follow(&probe)?;
    let glide = strengths_glide(&probe)?;
    anyhow::ensure!(dissolve && overlays && finish && glide, "see the FAIL lines above");

    println!("OK");
    Ok(())
}

/// A source that says in every frame which of its seconds is showing: the
/// picture's brightness is 20 + 6s (in whole levels), the tone 200 + 50s Hz.
/// And the shipped ffmpeg, to measure what came out.
struct Probe {
    ff: String,
    ramp: std::path::PathBuf,
}

impl Probe {
    fn new() -> anyhow::Result<Self> {
        let ff = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());
        let ramp = std::env::temp_dir().join("cutlass_export_ramp.mkv");
        let made = std::process::Command::new(&ff)
            .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
            .arg("color=c=black:s=320x180:r=30:d=24,format=yuv420p,geq=lum='20+6*T':cb=128:cr=128")
            .args(["-f", "lavfi", "-i", "aevalsrc='0.5*sin(2*PI*(200*t+25*t*t))':s=48000:d=24"])
            .args(["-c:v", "ffv1", "-c:a", "pcm_s16le"])
            .arg(&ramp)
            .status()?;
        anyhow::ensure!(made.success(), "could not make the ramp source");
        Ok(Self { ff, ramp })
    }
    fn path(&self) -> String {
        self.ramp.to_string_lossy().into_owned()
    }
    fn luma(s: f64) -> f64 {
        (20.0 + 6.0 * s).floor()
    }
    fn tone(s: f64) -> f64 {
        200.0 + 50.0 * s
    }
    /// ProRes at 640x360, every frame its own: H.264 holds a flat picture
    /// that changes by a level or two, which reads as the footage lagging.
    fn settings() -> ExportSettings {
        ExportSettings {
            width: 640,
            height: 360,
            fps: 30,
            format: cutlass_core::export::ExportFormat::MovProres,
            ..Default::default()
        }
    }
    /// Mean brightness of every frame of `out`, over the region `crop`
    /// (ffmpeg's crop=w:h:x:y), or the whole frame.
    fn frames(&self, out: &std::path::Path, crop: Option<&str>) -> anyhow::Result<Vec<f64>> {
        let crop = crop.map(|c| format!("crop={c},")).unwrap_or_default();
        let pix = std::process::Command::new(&self.ff)
            .args(["-v", "error", "-i"])
            .arg(out)
            .args(["-vf", &format!("{crop}extractplanes=y,scale=40:22:flags=area")])
            .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()?
            .stdout;
        Ok(pix
            .chunks_exact(40 * 22)
            .map(|f| f.iter().map(|&v| v as f64).sum::<f64>() / f.len() as f64)
            .collect())
    }
    /// `out`'s sound, mono at 48 kHz.
    fn sound(&self, out: &std::path::Path) -> anyhow::Result<Vec<i16>> {
        let pcm = std::process::Command::new(&self.ff)
            .args(["-v", "error", "-i"])
            .arg(out)
            .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"])
            .output()?
            .stdout;
        Ok(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
    }
}

/// The samples of `samples` from `t` for `dur` seconds.
fn window(samples: &[i16], t: f64, dur: f64) -> &[i16] {
    let from = ((t * 48000.0) as usize).min(samples.len());
    &samples[from..(from + (dur * 48000.0) as usize).min(samples.len())]
}

/// Pitch from zero crossings over 0.1s from `t`.
fn pitch_at(samples: &[i16], t: f64) -> f64 {
    let w = window(samples, t, 0.1);
    w.windows(2).filter(|p| (p[0] < 0) != (p[1] < 0)).count() as f64 / 2.0 / 0.1
}

/// Loudness (RMS) over `dur` seconds from `t`.
fn rms_at(samples: &[i16], t: f64, dur: f64) -> f64 {
    let w = window(samples, t, dur);
    (w.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

fn report(ok: bool, what: String) -> bool {
    println!("{} {what}", if ok { "PASS" } else { "FAIL" });
    ok
}

/// A dissolve between retimed clips plays each at its own speed, from where
/// the clip itself is. The windows used to be read at normal speed from
/// positions that ignored speed: out of a 2x clip, the picture jumped back
/// three seconds of footage and slowed to 1x for the dissolve.
fn dissolve_keeps_speed(probe: &Probe) -> anyhow::Result<bool> {
    use cutlass_core::export::{build_segments, ClipFx, ExportClip};
    // A: 4s at 2x from source 2s (2..10); B: 3s at half speed from 15s, with
    // a 1s dissolve into it, so its head is source 14.5..15.
    let clip = |start: f64, len: f64, src_in: f64, speed: f64, trans: f64| ExportClip {
        start,
        len,
        src_in,
        path: probe.path(),
        fx: ClipFx { speed, ..Default::default() },
        lut: String::new(),
        trans_dur: trans,
        trans_dip: false,
        kf: Default::default(),
    };
    let segs = build_segments(vec![clip(0.0, 4.0, 2.0, 2.0, 0.0), clip(4.0, 3.0, 15.0, 0.5, 1.0)]);
    let out = std::env::temp_dir().join("cutlass_export_dissolve_speed.mov");
    export(&segs, &[], &[], &out, &Probe::settings(), &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;

    // which source second each side shows at timeline t
    let a_src = |t: f64| 2.0 + 2.0 * t;
    let b_src = |t: f64| 15.0 + 0.5 * (t - 4.0);
    let expected_y = |t: f64| {
        if t < 3.0 {
            Probe::luma(a_src(t))
        } else if t < 4.0 {
            let p = t - 3.0; // the dissolve's progress
            (1.0 - p) * Probe::luma(a_src(t)) + p * Probe::luma(b_src(t))
        } else {
            Probe::luma(b_src(t))
        }
    };
    let frames = probe.frames(&out, None)?;
    let worst = furthest(&frames, |t| Some(expected_y(t)));
    let mut ok = report(
        frames.len() >= 205 && worst.0 < 3.0,
        format!(
            "dissolve picture at clip speed: {} frames, furthest {:.1} levels from expected at {:.2}s",
            frames.len(),
            worst.0,
            worst.1
        ),
    );
    // the sound: pitch in each clip's body and at the start of the
    // dissolve, where A is still 90% of the mix
    let samples = probe.sound(&out)?;
    for (t, want, what) in [
        (1.0, Probe::tone(a_src(1.05)), "A's body"),
        (3.0, Probe::tone(a_src(3.05)), "the dissolve's start"),
        (5.0, Probe::tone(b_src(5.05)), "B's body"),
    ] {
        let got = pitch_at(&samples, t);
        ok &= report((got - want).abs() < 25.0, format!("dissolve sound at clip speed, {what}: {got:.0} Hz, expected {want:.0}"));
    }
    Ok(ok)
}

/// The furthest `frames` (at 30 fps) are from `expected(t)`, and when;
/// frames it gives no expectation for are skipped.
fn furthest(frames: &[f64], expected: impl Fn(f64) -> Option<f64>) -> (f64, f64) {
    let mut worst = (0.0f64, 0.0f64);
    for (n, y) in frames.iter().enumerate() {
        let t = n as f64 / 30.0;
        if let Some(e) = expected(t) {
            if (y - e).abs() > worst.0 {
                worst = ((y - e).abs(), t);
            }
        }
    }
    worst
}

/// Clips above the base track, and on the audio tracks, export the settings
/// the Inspector gives them. They used to get movement and colour and
/// nothing else: a sped-up clip there played at normal speed, brightness,
/// contrast and blur keyframes held their first value, a LUT was dropped,
/// and a music bed's fades, speed and volume keyframes were ignored.
fn overlays_keep_their_settings(probe: &Probe) -> anyhow::Result<bool> {
    use cutlass_core::export::{build_segments, ClipFx, ExportClip};
    use std::collections::BTreeMap;
    let ff = &probe.ff;
    let dir = std::env::temp_dir();
    // a flat grey base (luma 60) with no sound, so only the clips above are
    // heard
    let grey = dir.join("cutlass_export_grey.mkv");
    let made = std::process::Command::new(ff)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=black:s=320x180:r=30:d=6,format=yuv420p,geq=lum=60:cb=128:cr=128")
        .args(["-c:v", "ffv1"])
        .arg(&grey)
        .status()?;
    anyhow::ensure!(made.success(), "could not make the grey base");
    // a LUT that halves every channel
    let cube = dir.join("cutlass_export_half.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                text.push_str(&format!("{} {} {}\n", r as f64 * 0.5, g as f64 * 0.5, b as f64 * 0.5));
            }
        }
    }
    std::fs::write(&cube, text)?;

    let base = build_segments(vec![ExportClip {
        start: 0.0,
        len: 6.0,
        src_in: 0.0,
        path: grey.to_string_lossy().into_owned(),
        fx: ClipFx::default(),
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
        kf: Default::default(),
    }]);
    let kf = |p: &str, pts: &[(f64, f64)]| -> BTreeMap<String, Vec<(f64, f64)>> {
        [(p.to_string(), pts.to_vec())].into_iter().collect()
    };
    // V2, 0.5..3.5: 2x from source 2s, brightened 0 -> 0.2 over its first 2s,
    // silent; V3, 4.0..5.5: the LUT, silent; A1, 0..3: a bed at 2x from
    // source 4s, its volume dipped to a quarter from 1.0s to 1.5s, faded out
    // over its last second.
    let overlays = vec![
        Overlay {
            path: probe.path(),
            src_in: 2.0,
            len: 3.0,
            start: 0.5,
            fx: ClipFx { speed: 2.0, volume: 0.0, ..Default::default() },
            audio_only: false,
            lut: String::new(),
            kf: kf("brightness", &[(0.0, 0.0), (2.0, 0.2)]),
        },
        Overlay {
            path: probe.path(),
            src_in: 10.0,
            len: 1.5,
            start: 4.0,
            fx: ClipFx { volume: 0.0, ..Default::default() },
            audio_only: false,
            lut: cube.to_string_lossy().into_owned(),
            kf: Default::default(),
        },
        Overlay {
            path: probe.path(),
            src_in: 4.0,
            len: 3.0,
            start: 0.0,
            fx: ClipFx { speed: 2.0, fade_out: 1.0, ..Default::default() },
            audio_only: true,
            lut: String::new(),
            kf: kf("volume", &[(1.0, 1.0), (1.5, 0.25)]),
        },
    ];
    let out = dir.join("cutlass_export_overlay_settings.mov");
    export(&base, &overlays, &[], &out, &Probe::settings(), &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;

    // lutyuv's brightness is a share of its top level, 235
    let v2 = |t: f64| {
        let u = t - 0.5;
        Probe::luma(2.0 + 2.0 * u) + 0.2 * (u / 2.0).min(1.0) * 235.0
    };
    // halved in RGB, which runs from limited range's 16
    let v3 = |t: f64| 16.0 + 0.5 * (Probe::luma(10.0 + (t - 4.0)) - 16.0);
    let frames = probe.frames(&out, None)?;
    let w = furthest(&frames, |t| (0.55..3.45).contains(&t).then(|| v2(t)));
    let mut ok = report(
        w.0 < 3.0,
        format!("V2 clip at 2x with keyframed brightness: furthest {:.1} levels from expected at {:.2}s", w.0, w.1),
    );
    let w = furthest(&frames, |t| (4.05..5.45).contains(&t).then(|| v3(t)));
    ok &= report(w.0 < 3.0, format!("V3 clip's LUT: furthest {:.1} levels from expected at {:.2}s", w.0, w.1));
    let w = furthest(&frames, |t| (t < 0.45 || (3.55..3.95).contains(&t) || t > 5.55).then_some(60.0));
    ok &= report(w.0 < 3.0, format!("base shows between them: furthest {:.1} levels from grey at {:.2}s", w.0, w.1));

    let samples = probe.sound(&out)?;
    for (t, what) in [(0.5, "before the dip"), (1.7, "in the dip")] {
        let want = Probe::tone(4.0 + 2.0 * (t + 0.05));
        let got = pitch_at(&samples, t);
        ok &= report((got - want).abs() < 25.0, format!("music bed at 2x, {what}: {got:.0} Hz, expected {want:.0}"));
    }
    let full = rms_at(&samples, 0.3, 0.5);
    let dipped = rms_at(&samples, 1.6, 0.35) / full;
    ok &= report((dipped - 0.25).abs() < 0.05, format!("music bed's volume keyframes: {dipped:.2} of full, expected 0.25"));
    let tail = rms_at(&samples, 2.85, 0.1) / full;
    ok &= report(tail < 0.2, format!("music bed's fade-out: {tail:.2} of full near its end"));
    let after = rms_at(&samples, 3.2, 0.5) / full;
    ok &= report(after < 0.01, format!("music bed ends with its clip: {after:.3} of full after it"));
    Ok(ok)
}

/// A tracked blur box follows its keyframes every frame, and a clip above V1
/// brings its vignette, grain and blur boxes to the frame while it's on
/// screen. The box used to move in 0.2s steps, so one tracking a face could
/// leave it showing between them; above V1 all three were left out.
fn boxes_and_finish_follow(probe: &Probe) -> anyhow::Result<bool> {
    use cutlass_core::export::{build_segments, CensorSlot, ClipFx, ExportClip};
    let dir = std::env::temp_dir();
    let grey = dir.join("cutlass_export_grey.mkv"); // made by overlays_keep_their_settings
    let grey_s = grey.to_string_lossy().into_owned();
    // V1, 6s of grey: a solid red box 0.2 of the frame across, tracked from
    // 0.2 to 0.8 of the width over its first 2s
    let mut fx = ClipFx::default();
    fx.censor[0] = CensorSlot { style: 3.0, y: 0.5, w: 0.2, h: 0.2, color: 0xFF0000 as f64, ..Default::default() };
    let segs = build_segments(vec![ExportClip {
        start: 0.0,
        len: 6.0,
        src_in: 0.0,
        path: grey_s.clone(),
        fx,
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
        kf: [("censor_x".to_string(), vec![(0.0, 0.2), (2.0, 0.8)])].into_iter().collect(),
    }]);
    // V2, 3s..5s, grey over grey: vignette, grain, and a blue box at the top left
    let mut ofx = ClipFx { vignette: 0.8, grain: 0.6, ..Default::default() };
    ofx.censor[0] = CensorSlot { style: 3.0, x: 0.25, y: 0.25, w: 0.2, h: 0.2, color: 0x0000FF as f64, ..Default::default() };
    let ov = Overlay {
        path: grey_s,
        src_in: 0.0,
        len: 2.0,
        start: 3.0,
        fx: ofx,
        audio_only: false,
        lut: String::new(),
        kf: Default::default(),
    };
    let out = dir.join("cutlass_export_finish.mov");
    export(&segs, &[ov], &[], &out, &Probe::settings(), &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;

    // every frame as RGB
    let (w, h) = (640usize, 360usize);
    let raw = std::process::Command::new(&probe.ff)
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vf", "scale=in_color_matrix=bt709:in_range=tv,format=rgb24", "-f", "rawvideo", "-"])
        .output()?
        .stdout;
    let frames: Vec<&[u8]> = raw.chunks_exact(w * h * 3).collect();
    let px = |f: &[u8], x: usize, y: usize| [f[(y * w + x) * 3], f[(y * w + x) * 3 + 1], f[(y * w + x) * 3 + 2]];
    let mut ok = true;

    // the red box's middle, frame by frame, against where its keyframes put it
    let mut worst = (0.0f64, 0usize);
    for (n, f) in frames.iter().enumerate().take(60) {
        let red: Vec<usize> = (0..w).filter(|&x| {
            let [r, g, b] = px(f, x, h / 2);
            r > 150 && g < 80 && b < 80
        }).collect();
        let want = (0.2 + 0.6 * n as f64 / 60.0) * w as f64;
        let got = red.first().zip(red.last()).map(|(a, b)| (a + b) as f64 / 2.0).unwrap_or(f64::NAN);
        let off = (got - want).abs();
        if !(off <= worst.0) {
            worst = (off, n);
        }
    }
    ok &= report(worst.0 <= 3.0, format!("tracked box follows every frame: furthest {:.1}px from its keyframes (frame {})", worst.0, worst.1));

    // the frame while the V2 clip is on (4.0s) and after it (5.5s)
    let mean = |f: &[u8], x0: usize, y0: usize, n: usize| {
        (0..n * n).map(|i| px(f, x0 + i % n, y0 + i / n)[1] as f64).sum::<f64>() / (n * n) as f64
    };
    let noise = |a: &[u8], b: &[u8]| {
        // frame-to-frame change over a flat patch: grain moves
        (0..1600).map(|i| (px(a, 400 + i % 40, 280 + i / 40)[1] as f64 - px(b, 400 + i % 40, 280 + i / 40)[1] as f64).abs()).sum::<f64>() / 1600.0
    };
    for (t, on) in [(4.0, true), (5.5, false)] {
        let n = (t * 30.0) as usize;
        let (f, g) = (frames[n], frames[n + 1]);
        let darker = mean(f, w / 2 - 10, h / 2 + 60, 20) - mean(f, 0, h - 20, 20);
        let blue = { let [r, _, b] = px(f, w / 4, h / 4); b > 150 && r < 80 };
        let moved = noise(f, g);
        let what = if on { "while the V2 clip is on" } else { "after it" };
        ok &= report((darker > 15.0) == on, format!("V2 vignette {what}: corner {darker:.0} levels darker than the middle"));
        ok &= report((moved > 2.0) == on, format!("V2 grain {what}: {moved:.1} levels of change frame to frame"));
        ok &= report(blue == on, format!("V2 blur box {what}: {}", if blue { "there" } else { "not there" }));
    }
    Ok(ok)
}


/// Grain, sharpen and a blur box's size follow their keyframes every frame,
/// on V1 and above it. Their filters can't change once running, so they used
/// to be sampled into 0.2s steps: a fade-up of grain or a growing box moved
/// five times a second.
///
/// The source is flat grey above (grain shows as spread, a box as its own
/// colour) and a fine texture below (sharpen raises its contrast, blur
/// flattens it). A value that glides changes at nearly every frame of its
/// ramp; a stepped one sits still for six frames at a time.
fn strengths_glide(probe: &Probe) -> anyhow::Result<bool> {
    use cutlass_core::export::{build_segments, CensorSlot, ClipFx, ExportClip};
    use std::collections::BTreeMap;
    let ff = &probe.ff;
    let dir = std::env::temp_dir();
    let (w, h) = (640usize, 360usize);
    let src = dir.join("cutlass_export_texture.mkv");
    let made = std::process::Command::new(ff)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=black:s=640x360:r=30:d=10,format=yuv420p,geq=lum='if(lt(Y,180),110,110+60*sin(X*1.7)*sin(Y*1.3))':cb=128:cr=128")
        .args(["-c:v", "ffv1"])
        .arg(&src)
        .status()?;
    anyhow::ensure!(made.success(), "could not make the texture");
    let src_s = src.to_string_lossy().into_owned();
    let kf = |p: &str, pts: &[(f64, f64)]| -> BTreeMap<String, Vec<(f64, f64)>> {
        [(p.to_string(), pts.to_vec())].into_iter().collect()
    };
    let clip = |start: f64, fx: ClipFx, kf: BTreeMap<String, Vec<(f64, f64)>>| ExportClip {
        start,
        len: 2.0,
        src_in: start,
        path: src_s.clone(),
        fx,
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
        kf,
    };
    // 4..6s: a solid red box up top and a blur box below, both growing from
    // 0.1 to 0.5 of the width
    let mut boxes = ClipFx::default();
    boxes.censor[0] = CensorSlot { style: 3.0, y: 0.25, h: 0.2, color: 0xFF0000 as f64, ..Default::default() };
    boxes.censor[1] = CensorSlot { style: 1.0, y: 0.75, h: 0.2, strength: 0.5, ..Default::default() };
    let mut box_kf = kf("censor_w", &[(0.0, 0.1), (2.0, 0.5)]);
    box_kf.insert("censor2_w".into(), vec![(0.0, 0.1), (2.0, 0.5)]);
    let segs = build_segments(vec![
        clip(0.0, ClipFx::default(), kf("grain", &[(0.0, 0.0), (2.0, 0.6)])),
        clip(2.0, ClipFx::default(), kf("sharpen", &[(0.0, 0.0), (2.0, 1.0)])),
        clip(4.0, boxes, box_kf),
        clip(6.0, ClipFx::default(), Default::default()),
        clip(8.0, ClipFx::default(), Default::default()),
    ]);
    // on V2 over the same picture: 6..8s grain ramping (and sharpen, which
    // only has to render here), 8..10s a blur box growing
    let mut okf = kf("grain", &[(0.0, 0.0), (2.0, 0.6)]);
    okf.insert("sharpen".into(), vec![(0.0, 0.0), (2.0, 0.5)]);
    let mut bfx = ClipFx::default();
    bfx.censor[0] = CensorSlot { style: 1.0, y: 0.75, h: 0.2, strength: 0.5, ..Default::default() };
    let upper = |start: f64, fx: ClipFx, kf: BTreeMap<String, Vec<(f64, f64)>>| Overlay {
        path: src_s.clone(),
        src_in: start,
        len: 2.0,
        start,
        fx,
        audio_only: false,
        lut: String::new(),
        kf,
    };
    let ovs = [upper(6.0, ClipFx::default(), okf), upper(8.0, bfx, kf("censor_w", &[(0.0, 0.1), (2.0, 0.5)]))];
    let out = dir.join("cutlass_export_glide.mov");
    export(&segs, &ovs, &[], &out, &Probe::settings(), &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;

    let raw = std::process::Command::new(ff)
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vf", "scale=in_color_matrix=bt709:in_range=tv,format=rgb24", "-f", "rawvideo", "-"])
        .output()?
        .stdout;
    let frames: Vec<&[u8]> = raw.chunks_exact(w * h * 3).collect();
    anyhow::ensure!(frames.len() >= 299, "the render has {} frames", frames.len());
    let g = |f: &[u8], x: usize, y: usize| f[(y * w + x) * 3 + 1] as f64;
    // spread of the flat grey (grain), and of the texture (sharpen)
    let spread = |f: &[u8], y0: usize| {
        let v: Vec<f64> = (0..120 * 60).map(|i| g(f, 40 + i % 120, y0 + i / 120)).collect();
        let m = v.iter().sum::<f64>() / v.len() as f64;
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    };
    // a box's width on row y: the run of red, or of flattened texture
    let red_width = |f: &[u8], y: usize| (0..w).filter(|&x| f[(y * w + x) * 3] > 150 && g(f, x, y) < 80.0).count() as f64;
    let blurred_width = |f: &[u8], y: usize| {
        // where the texture's pixel-to-pixel change is gone
        (2..w - 2).filter(|&x| (g(f, x + 1, y) - g(f, x, y)).abs() + (g(f, x, y + 1) - g(f, x, y)).abs() < 3.0).count() as f64
    };
    let mut ok = true;
    let mut ramp = |what: &str, from: usize, measure: &dyn Fn(&[u8]) -> f64, want: (f64, f64)| {
        let v: Vec<f64> = (from + 2..from + 58).map(|n| measure(frames[n])).collect();
        // moves at (nearly) every frame of the ramp, and from where to where
        let moves = v.windows(2).filter(|p| p[1] > p[0] + 0.01).count();
        let (a, b) = (v[0], v[v.len() - 1]);
        let pass = moves >= v.len() * 3 / 4 && a < want.0 && b > want.1;
        println!(
            "{} {what} glides: rises at {moves} of {} frames, {a:.1} to {b:.1}",
            if pass { "PASS" } else { "FAIL" },
            v.len() - 1
        );
        ok &= pass;
    };
    // (where it starts below, where it ends above)
    ramp("V1 grain", 0, &|f| spread(f, 40), (1.5, 4.0));
    ramp("V1 sharpen", 60, &|f| spread(f, 240), (40.0, 60.0));
    ramp("V1 solid box width", 120, &|f| red_width(f, h / 4), (90.0, 290.0));
    ramp("V1 blur box width", 120, &|f| blurred_width(f, h * 3 / 4), (90.0, 290.0));
    ramp("V2 grain", 180, &|f| spread(f, 40), (1.5, 4.0));
    ramp("V2 blur box width", 240, &|f| blurred_width(f, h * 3 / 4), (90.0, 290.0));
    Ok(ok)
}
