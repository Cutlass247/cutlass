//! End-to-end check of the import pipeline without the UI:
//! `cargo run -p cutlass-core --example import_check -- <video>`

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: import_check <video>"))?;
    cutlass_core::media::ensure_ffmpeg()?;

    let t0 = std::time::Instant::now();
    let info = cutlass_core::media::import(Path::new(&path))?;
    println!(
        "imported {} — {:.1}s, {} scrub frames @ {:.2} fps, {} waveform buckets in {:.1}s",
        info.name,
        info.duration_s,
        info.thumb_paths.len(),
        info.scrub_fps,
        info.waveform.len(),
        t0.elapsed().as_secs_f64()
    );

    let mut project = cutlass_core::project::Project::new("ImportCheck");
    project.add_clip(&cutlass_core::project::Clip {
        id: "c1".into(),
        name: info.name.clone(),
        media: info.id.clone(),
        track: "V1".into(),
        start: 0.0,
        len: info.duration_s,
        src_in: 0.0,
    })?;
    let snap = project.snapshot();
    assert_eq!(snap["clips"][0]["media"], serde_json::json!(info.id));
    println!("project snapshot OK: {}", snap);
    Ok(())
}
