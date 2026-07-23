//! Verify LUT export: write a tiny .cube, apply it to a clip, and confirm
//! the lut3d filter (with Windows-path escaping) renders a valid MP4.
//! `cargo run -p cutlass-core --example lut_check -- <clip>`

use std::io::Write;

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: lut_check <clip>");
    cutlass_core::media::ensure_ffmpeg()?;

    // a 2x2x2 LUT that nudges toward warm (red fastest ordering)
    let cube = std::env::temp_dir().join("cutlass_test.cube");
    {
        let mut f = std::fs::File::create(&cube)?;
        writeln!(f, "LUT_3D_SIZE 2")?;
        for v in [
            "0 0 0", "1 0 0", "0 1 0", "1 1 0", "0 0 0.8", "1 0 0.8", "0 1 0.8", "1 1 0.8",
        ] {
            writeln!(f, "{v}")?;
        }
    }

    let segs = vec![Segment::Clip {
        path: a,
        src_in: 0.0,
        len: 3.0,
        fx: ClipFx::default(),
        lut: cube.to_string_lossy().to_string(),
    }];
    let out = std::env::temp_dir().join("cutlass_lut_check.mp4");
    let settings = ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() };

    let enc = export(&segs, &[], &[], &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
    let dur = probe_duration_s(&out)?;
    println!("lut render: encoder={enc} dur={dur:.2}s -> {}", out.display());
    assert!((dur - 3.0).abs() < 0.3, "duration off: {dur}");
    println!("OK lut3d render");
    Ok(())
}
