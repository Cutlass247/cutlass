//! Verify the staged (chunked) export path: a timeline long enough to be
//! rendered in parts must come out the same length, in sync, with overlays and
//! titles landing where they belong even when they straddle a part boundary.
//!
//! `cargo run -p cutlass-core --example staged_check -- <markers.mp4> <red.mp4> <outdir>`
//!
//! `markers.mp4` carries a white flash and a beep in the first 100 ms of every
//! second, so the caller can measure video and audio onsets independently.

use cutlass_core::export::{export, ClipFx, ExportSettings, Overlay, Segment, Title};

const N: usize = 24; // > CHUNK_MIN_SEGMENTS, so staging engages
const SEG: f64 = 1.0;

fn settings() -> ExportSettings {
    ExportSettings {
        width: 1280,
        height: 720,
        fps: 30,
        ..Default::default()
    }
}

fn segments(src: &str) -> Vec<Segment> {
    (0..N)
        .map(|k| Segment::Clip {
            path: src.to_string(),
            src_in: k as f64 * SEG,
            len: SEG,
            fx: ClipFx::default(),
            lut: String::new(),
        })
        .collect()
}

fn run(name: &str, segs: &[Segment], ovs: &[Overlay], tls: &[Title], out: &std::path::Path) {
    let mut last = 0.0f32;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut seen_backwards = false;
    let t = std::time::Instant::now();
    let res = export(
        segs,
        ovs,
        tls,
        out,
        &settings(),
        &mut |p| {
            if p + 1e-6 < last {
                seen_backwards = true;
            }
            last = p;
        },
        &cancel,
    );
    match res {
        Ok(enc) => println!(
            "{name}: ok encoder={enc} {:.1}s progress_end={last:.3} monotonic={}",
            t.elapsed().as_secs_f64(),
            !seen_backwards
        ),
        Err(e) => println!("{name}: FAILED {e:#}"),
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let markers = a
        .next()
        .expect("usage: staged_check <markers> <red> <outdir>");
    let red = a.next().expect("need red source");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    cutlass_core::media::ensure_ffmpeg()?;

    // 1. sync: nothing but cuts, so flashes and beeps stay measurable
    run(
        "plain",
        &segments(&markers),
        &[],
        &[],
        &dir.join("staged_plain.mp4"),
    );

    // 2. an overlay and a title that each straddle a part boundary
    //    (boundaries fall at 8s and 16s with 8 segments per part)
    let ov = Overlay {
        path: red,
        src_in: 0.0,
        len: 2.0,
        start: 15.5,
        fx: ClipFx::default(),
        audio_only: false,
    };
    let ti = Title {
        text: "BOUNDARY".into(),
        start: 7.5,
        len: 2.0,
        pos_x: 0.0,
        pos_y: 0.0,
        font_size: 96.0,
        bg: 0.0,
    };
    run(
        "sliced",
        &segments(&markers),
        &[ov],
        &[ti],
        &dir.join("staged_sliced.mp4"),
    );

    // 3. same timeline just under the staging threshold, as the control
    let short: Vec<Segment> = segments(&markers).into_iter().take(8).collect();
    run(
        "single_pass",
        &short,
        &[],
        &[],
        &dir.join("single_pass.mp4"),
    );
    Ok(())
}
