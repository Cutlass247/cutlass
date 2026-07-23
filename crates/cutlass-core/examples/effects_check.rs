//! Render a clip with the new stylize effects (hue, blur, vignette, flip)
//! stacked and confirm the filtergraph produces a valid file.
//! `cargo run -p cutlass-core --example effects_check -- <clip>`

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: effects_check <clip>");
    cutlass_core::media::ensure_ffmpeg()?;

    let fx = ClipFx {
        contrast: 1.2,
        saturation: 1.3,
        temperature: 40.0,
        tint: -20.0,
        hue: 40.0,
        blur: 5.0,
        sharpen: 0.6,
        grain: 0.4,
        vignette: 0.5,
        flip_h: 1.0,
        denoise: 1.0,
        ..Default::default()
    };
    let segs = vec![Segment::Clip { path: a, src_in: 0.0, len: 3.0, fx, lut: String::new() }];
    let out = std::env::temp_dir().join("cutlass_effects_check.mp4");
    let settings = ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() };

    let enc = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
    let dur = probe_duration_s(&out)?;
    println!("effects render: encoder={enc} dur={dur:.2}s -> {}", out.display());
    assert!((dur - 3.0).abs() < 0.3, "duration off: {dur}");
    println!("OK effects (hue + blur + vignette + mirror) render");
    Ok(())
}
