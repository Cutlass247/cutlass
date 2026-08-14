//! End-to-end reframe export: take a real landscape clip through the actual
//! `export()` pipeline into a vertical (9:16) output, for each reframe mode.
//! Self-skips unless FFMPEG_BINARY points at the shipped (LGPL) ffmpeg.
//! Outputs land in REFRAME_OUT_DIR (default: temp) for inspection.

use cutlass_core::export::{
    build_segments, export, ClipFx, ExportClip, ExportFormat, ExportSettings, Quality, Reframe,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn out_dir() -> PathBuf {
    std::env::var("REFRAME_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// Generate a 4s 1280x720 landscape source with a distinctive pattern + tone,
/// encoded with the shipped LGPL encoder so the whole path stays LGPL-only.
fn make_source(dir: &PathBuf, ff: &str) -> PathBuf {
    let src = dir.join("reframe_src_landscape.mp4");
    // testsrc2 gives clear coloured zones (so crop-vs-fit is obvious); the
    // added film grain makes the frame incompressible enough that the real
    // encoder fills the requested bitrate (otherwise export()'s under-target
    // guard rejects synthetic footage as if the encoder ignored -b:v).
    let status = std::process::Command::new(ff)
        .args([
            "-y",
            "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=4,noise=alls=22:allf=t+u",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=4",
            "-c:v", "libopenh264", "-b:v", "20M", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-shortest",
        ])
        .arg(&src)
        .status()
        .expect("spawn ffmpeg for source");
    assert!(status.success(), "source generation failed");
    src
}

fn export_reframe(src: &PathBuf, out: &PathBuf, reframe: Reframe) -> String {
    let clip = ExportClip {
        start: 0.0,
        len: 4.0,
        src_in: 0.0,
        path: src.to_string_lossy().into_owned(),
        fx: ClipFx::default(),
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
        kf: BTreeMap::new(),
    };
    let segments = build_segments(vec![clip]);
    let settings = ExportSettings {
        width: 1080,
        height: 1920,
        fps: 30,
        format: ExportFormat::parse("mp4_h264"),
        quality: Quality::parse("medium"),
        reframe,
        reframe_x: 0.5,
        reframe_y: 0.5,
    };
    let cancel = AtomicBool::new(false);
    let mut progress = |_p: f32| {};
    export(&segments, &[], &[], out, &settings, &mut progress, &cancel)
        .unwrap_or_else(|e| panic!("export {out:?} failed: {e:#}"))
}

#[test]
fn reframe_modes_export_vertical() {
    let Ok(ff) = std::env::var("FFMPEG_BINARY") else {
        eprintln!("skipping: set FFMPEG_BINARY to the shipped ffmpeg to run this");
        return;
    };
    let dir = out_dir();
    let src = make_source(&dir, &ff);

    for (name, mode) in [
        ("reframe_letterbox.mp4", Reframe::parse("letterbox")),
        ("reframe_fill.mp4", Reframe::parse("fill")),
        ("reframe_blur.mp4", Reframe::parse("blur")),
    ] {
        let out = dir.join(name);
        let enc = export_reframe(&src, &out, mode);
        assert!(out.exists(), "{name} was not produced");
        let bytes = std::fs::metadata(&out).unwrap().len();
        assert!(bytes > 10_000, "{name} is suspiciously small ({bytes} bytes)");
        eprintln!("OK {name}: {bytes} bytes, encoder={enc}");
    }
}
