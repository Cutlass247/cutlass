//! Audit harness: push the export at its edges and report what breaks.
//! `cargo run -p cutlass-core --example limits_check -- <clip> <outdir>`

use cutlass_core::export::{export, ClipFx, ExportFormat, ExportSettings, Quality, Segment};

fn settings(format: ExportFormat) -> ExportSettings {
    ExportSettings {
        width: 640,
        height: 360,
        fps: 30,
        format,
        quality: Quality::Low,
        ..Default::default()
    }
}

/// A clip with a full grade on it — the realistic case, and the one that makes
/// each clip's slice of the filtergraph longest.
fn graded() -> ClipFx {
    ClipFx {
        brightness: 0.1,
        contrast: 1.1,
        saturation: 1.2,
        temperature: 30.0,
        tint: -10.0,
        vignette: 0.3,
        grain: 0.2,
        sharpen: 0.3,
        ..Default::default()
    }
}

fn run(name: &str, segs: &[Segment], fmt: ExportFormat, out: &std::path::Path) {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let t = std::time::Instant::now();
    match export(segs, &[], &[], out, &settings(fmt), &mut |_| {}, &cancel) {
        Ok(enc) => {
            let dur = cutlass_core::media::probe_duration_s(out).unwrap_or(-1.0);
            println!("  PASS {name}: encoder={enc} dur={dur:.2}s in {:.1}s", t.elapsed().as_secs_f64());
        }
        Err(e) => println!("  FAIL {name}: {e:#}"),
    }
}

fn clips(src: &str, n: usize, len: f64, fx: ClipFx) -> Vec<Segment> {
    (0..n)
        .map(|k| Segment::Clip {
            path: src.to_string(),
            src_in: (k as f64 * 0.05) % 5.0,
            len,
            fx: fx.clone(),
            lut: String::new(),
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let src = a.next().expect("usage: limits_check <clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    // 1. Windows caps a command line at 32767 chars. MP4 is staged into parts
    //    of 8 so its graph stays small, but ProRes and WebM are not staged --
    //    a many-cut timeline builds one enormous filter_complex.
    println!("== many clips, formats that are NOT staged ==");
    for n in [40usize, 120, 240] {
        let segs = clips(&src, n, 0.1, graded());
        run(&format!("{n} graded clips -> WebM"), &segs, ExportFormat::WebmVp9, &dir.join(format!("w{n}.webm")));
    }

    // 2. same counts through the staged MP4 path, as the control
    println!("== same, through the staged MP4 path ==");
    for n in [120usize, 240] {
        let segs = clips(&src, n, 0.1, graded());
        run(&format!("{n} graded clips -> MP4"), &segs, ExportFormat::Mp4H264, &dir.join(format!("m{n}.mp4")));
    }

    // 3. degenerate clip lengths
    println!("== degenerate spans ==");
    run("single 10ms clip", &clips(&src, 1, 0.01, ClipFx::default()), ExportFormat::Mp4H264, &dir.join("tiny.mp4"));
    run("20 x 10ms clips", &clips(&src, 20, 0.01, ClipFx::default()), ExportFormat::Mp4H264, &dir.join("tiny20.mp4"));
    Ok(())
}
