//! Hardening probe: render a 2s segment of a clip (with a couple effects)
//! at 1080p and report OK or the failure. Catches export breakage on
//! unusual inputs (odd aspect, hevc/vp9 sources, no-audio, high fps).
//! `cargo run -p cutlass-core --example render_probe -- <clip>`

use std::sync::atomic::AtomicBool;

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};
use cutlass_core::media::probe_duration_s;

fn main() {
    let path = std::env::args().nth(1).expect("usage: render_probe <clip>");
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match run(&path) {
        Ok(msg) => println!("OK    {name}  {msg}"),
        Err(e) => println!("FAIL  {name}  {e:#}"),
    }
}

fn run(path: &str) -> anyhow::Result<String> {
    cutlass_core::media::ensure_ffmpeg()?;
    let fx = ClipFx { contrast: 1.1, temperature: 20.0, vignette: 0.2, ..Default::default() };
    let segs = vec![Segment::Clip {
        path: path.to_string(),
        src_in: 0.0,
        len: 2.0,
        fx,
        lut: String::new(),
    }];
    let out = std::env::temp_dir().join("cutlass_render_probe.mp4");
    let settings = ExportSettings { width: 1920, height: 1080, fps: 30, ..Default::default() };
    let enc = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &AtomicBool::new(false))?;
    let dur = probe_duration_s(&out)?;
    let kb = std::fs::metadata(&out)?.len() / 1024;
    anyhow::ensure!((dur - 2.0).abs() < 0.4, "duration off: {dur:.2}");
    anyhow::ensure!(kb > 10, "suspiciously small output ({kb} KB)");
    Ok(format!("enc={enc} dur={dur:.2}s {kb}KB"))
}
