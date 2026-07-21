//! Verify multi-track export end-to-end: a base program (V1), a second
//! video track composited as picture-in-picture, and an audio-only bed
//! (an A-track) all render into one MP4 with a valid A/V stream and the
//! base program's duration.
//! `cargo run -p cutlass-core --example multitrack_check -- <clipA> <clipB>`

use cutlass_core::export::{export, has_audio, ClipFx, ExportSettings, Overlay, Segment, Title};
use cutlass_core::media::probe_duration_s;

fn main() -> anyhow::Result<()> {
    let a = std::env::args().nth(1).expect("usage: multitrack_check <clipA> <clipB>");
    let b = std::env::args().nth(2).expect("usage: multitrack_check <clipA> <clipB>");
    cutlass_core::media::ensure_ffmpeg()?;

    // base program (V1): 6s of clip A
    let segments = vec![Segment::Clip { path: a.clone(), src_in: 0.0, len: 6.0, fx: ClipFx::default() }];

    // V2: a scaled-down picture-in-picture of clip B over 2s..5s (video +
    // its audio). A1: an audio-only bed of clip B across the whole program
    // (its picture must NOT appear; only its audio mixes in at half gain).
    let overlays = vec![
        Overlay {
            path: b.clone(),
            src_in: 0.0,
            len: 3.0,
            start: 2.0,
            fx: ClipFx { scale: 0.4, pos_x: 0.25, pos_y: -0.25, ..Default::default() },
            audio_only: false,
        },
        Overlay {
            path: b.clone(),
            src_in: 1.0,
            len: 6.0,
            start: 0.0,
            fx: ClipFx { volume: 0.5, ..Default::default() },
            audio_only: true,
        },
    ];

    let out = std::env::temp_dir().join("cutlass_multitrack_check.mp4");
    let settings = ExportSettings { width: 1280, height: 720, fps: 30 };
    let no_titles: &[Title] = &[];
    export(&segments, &overlays, no_titles, &out, &settings, &mut |_p| {})?;

    let dur = probe_duration_s(&out)?;
    let audio = has_audio(&out.to_string_lossy());
    println!("multitrack render {dur:.2}s, audio={audio} -> {}", out.display());
    assert!((dur - 6.0).abs() < 0.2, "duration off: expected ~6.0, got {dur}");
    assert!(audio, "output has no audio stream — the bed/overlay audio didn't mix");
    println!("OK");
    Ok(())
}
