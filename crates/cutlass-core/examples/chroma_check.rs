//! Verify chroma-key export: a base clip with a chroma-keyed overlay
//! composites into one valid MP4 (colorkey + alpha overlay path).
//! `cargo run -p cutlass-core --example chroma_check -- <base> <overlay>`

use cutlass_core::export::{export, ClipFx, ExportSettings, Overlay, Segment, Title};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: chroma_check <base> <overlay>");
    let b = std::env::args().nth(2).expect("usage: chroma_check <base> <overlay>");
    cutlass_core::media::ensure_ffmpeg()?;

    let segs = vec![Segment::Clip { path: a, src_in: 0.0, len: 3.0, fx: ClipFx::default() }];
    let overlays = vec![Overlay {
        path: b,
        src_in: 0.0,
        len: 3.0,
        start: 0.0,
        fx: ClipFx { chroma: 1.0, chroma_sim: 0.35, ..Default::default() },
        audio_only: false,
    }];
    let out = std::env::temp_dir().join("cutlass_chroma_check.mp4");
    let settings = ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() };
    let no_titles: &[Title] = &[];

    let enc = export(&segs, &overlays, no_titles, &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
    let dur = probe_duration_s(&out)?;
    println!("chroma render: encoder={enc} dur={dur:.2}s -> {}", out.display());
    assert!((dur - 3.0).abs() < 0.3, "duration off: {dur}");
    println!("OK chroma key (colorkey overlay) render");
    Ok(())
}
