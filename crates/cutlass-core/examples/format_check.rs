//! Render a short clip to every export format and confirm each produces a
//! valid file with the right duration.
//! `cargo run -p cutlass-core --example format_check -- <clip>`

use cutlass_core::export::{export, ClipFx, ExportFormat, ExportSettings, Quality, Segment};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: format_check <clip>");
    cutlass_core::media::ensure_ffmpeg()?;
    let segs = vec![Segment::Clip { path: a, src_in: 0.0, len: 3.0, fx: ClipFx::default(), lut: String::new() }];

    for (fmt, name) in [
        (ExportFormat::Mp4H264, "h264"),
        (ExportFormat::Mp4H265, "h265"),
        (ExportFormat::MovProres, "prores"),
        (ExportFormat::WebmVp9, "vp9"),
    ] {
        let out = std::env::temp_dir().join(format!("cutlass_fmt_{name}.{}", fmt.ext()));
        let settings = ExportSettings {
            width: 640,
            height: 360,
            fps: 30,
            format: fmt,
            quality: Quality::Medium,
        };
        let enc = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
        let dur = probe_duration_s(&out)?;
        println!("{name}: encoder={enc} dur={dur:.2}s -> {}", out.display());
        assert!((dur - 3.0).abs() < 0.3, "{name} duration off: {dur}");
    }
    println!("OK all formats");
    Ok(())
}
