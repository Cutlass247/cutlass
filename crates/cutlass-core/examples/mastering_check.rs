//! Audit harness for audio mastering: export the same timeline with the
//! mastering off and on, so the two can be measured against each other.
//!
//! `cargo run -p cutlass-core --example mastering_check -- <voice> <music> <outdir>`
//!
//! `voice` carries speech-band bursts with gaps; `music` is a continuous bed at
//! a different frequency, so the bed can be isolated in the finished mix.

use cutlass_core::export::{export, ClipFx, ExportSettings, Overlay, Segment};

fn settings(master: bool) -> ExportSettings {
    ExportSettings {
        width: 640,
        height: 360,
        fps: 30,
        master_audio: master,
        ..Default::default()
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let voice = a
        .next()
        .expect("usage: mastering_check <voice> <music> <outdir>");
    let music = a.next().expect("need music");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    let segs = vec![Segment::Clip {
        path: voice,
        src_in: 0.0,
        len: 8.0,
        fx: ClipFx::default(),
        lut: String::new(),
    }];
    // a music bed under the whole thing, on an audio-only track
    let beds = vec![Overlay {
        path: music.clone(),
        src_in: 0.0,
        len: 8.0,
        start: 0.0,
        fx: ClipFx::default(),
        audio_only: true,
    }];

    for (label, master) in [("plain", false), ("mastered", true)] {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let out = dir.join(format!("{label}.mp4"));
        // voice alone, and voice under a music bed
        match export(
            &segs,
            &[],
            &[],
            &out,
            &settings(master),
            &mut |_| {},
            &cancel,
        ) {
            Ok(_) => println!("  {label}: ok -> {}", out.display()),
            Err(e) => println!("  {label}: FAILED {e:#}"),
        }
        let out_bed = dir.join(format!("{label}_bed.mp4"));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        match export(
            &segs,
            &beds,
            &[],
            &out_bed,
            &settings(master),
            &mut |_| {},
            &cancel,
        ) {
            Ok(_) => println!("  {label}+bed: ok -> {}", out_bed.display()),
            Err(e) => println!("  {label}+bed: FAILED {e:#}"),
        }
    }
    // Staged path: loudness is applied once at the join, while ducking and
    // levelling happen per part. If that split were wrong, each part would
    // carry its own gain and the level would step at every boundary — so this
    // renders a constant-level source across enough segments to stage, for the
    // caller to measure window by window.
    let flat: Vec<Segment> = (0..24)
        .map(|k| Segment::Clip {
            path: music.clone(),
            src_in: (k % 6) as f64,
            len: 1.0,
            fx: ClipFx::default(),
            lut: String::new(),
        })
        .collect();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let out = dir.join("staged_mastered.mp4");
    match export(&flat, &[], &[], &out, &settings(true), &mut |_| {}, &cancel) {
        Ok(_) => println!(
            "  staged+mastered: ok ({} segments) -> {}",
            flat.len(),
            out.display()
        ),
        Err(e) => println!("  staged+mastered: FAILED {e:#}"),
    }
    Ok(())
}
