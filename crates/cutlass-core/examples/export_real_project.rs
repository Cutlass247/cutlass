//! Render a real .cutlass file through the export path, so a project that
//! failed for a user can be reproduced exactly rather than approximated.
//! `cargo run -p cutlass-core --example export_real_project -- <file.cutlass> <out.mp4> [max_seconds]`

use std::collections::HashMap;

use cutlass_core::export::{
    build_segments, export, ClipFx, ExportClip, ExportSettings, Overlay, Segment,
};
use cutlass_core::project::Project;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let proj = a.next().expect("usage: export_real_project <file.cutlass> <out.mp4> [max_s]");
    let out = std::path::PathBuf::from(a.next().expect("need an output path"));
    let cap: f64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(f64::MAX);
    cutlass_core::media::ensure_ffmpeg()?;

    let mut p = Project::load(&std::fs::read(&proj)?)?;
    let paths: HashMap<String, String> = p
        .media_entries()
        .into_iter()
        .map(|(id, _, path, _)| (id, path))
        .collect();
    let clips = p.clips_state();

    // V1 is the programme; higher video tracks and audio tracks are overlays.
    let mut v1: Vec<ExportClip> = Vec::new();
    let mut overlays: Vec<Overlay> = Vec::new();
    for c in &clips {
        let Some(path) = paths.get(&c.media) else { continue };
        if c.start >= cap {
            continue;
        }
        let len = c.len.min(cap - c.start);
        if c.track == "V1" {
            v1.push(ExportClip {
                start: c.start,
                len,
                src_in: c.src_in,
                path: path.clone(),
                fx: ClipFx::from_map(&c.fx),
                kf: c
                    .kf
                    .keys()
                    .map(|k| (k.clone(), cutlass_core::project::kf_points(&c.kf, k)))
                    .collect(),
                lut: c.lut.clone(),
                trans_dur: 0.0,
                trans_dip: false,
            });
        } else {
            overlays.push(Overlay {
                path: path.clone(),
                src_in: c.src_in,
                len,
                start: c.start,
                fx: ClipFx::from_map(&c.fx),
                audio_only: c.track.starts_with('A'),
            });
        }
    }
    let segs = build_segments(v1);
    let total: f64 = segs
        .iter()
        .map(|s| match s {
            Segment::Clip { len, .. } | Segment::Gap { len } => *len,
            Segment::Transition { dur, .. } => *dur,
        })
        .sum();
    let tiny: Vec<f64> = segs
        .iter()
        .filter_map(|s| match s {
            Segment::Gap { len } if *len < 0.5 => Some(*len),
            _ => None,
        })
        .collect();
    println!("segments={} total={total:.1}s overlays={}", segs.len(), overlays.len());
    println!("sub-half-second gaps (each one used to hang the export): {tiny:?}");

    let settings = ExportSettings { width: 1920, height: 1080, fps: 30, ..Default::default() };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let t = std::time::Instant::now();
    let mut peak = 0.0f32;
    let res = export(&segs, &overlays, &[], &out, &settings, &mut |p| {
        if p > peak {
            peak = p;
            if (p * 100.0) as u32 % 5 == 0 {
                println!("  {:.0}% at {:.0}s", p * 100.0, t.elapsed().as_secs_f64());
            }
        }
    }, &cancel);
    match res {
        Ok(enc) => println!("DONE via {enc} in {:.0}s, reached {:.1}%", t.elapsed().as_secs_f64(), peak * 100.0),
        Err(e) => println!("FAILED at {:.1}% after {:.0}s: {e:#}", peak * 100.0, t.elapsed().as_secs_f64()),
    }
    Ok(())
}
