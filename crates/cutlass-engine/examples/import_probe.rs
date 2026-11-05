//! Hardening probe: run one clip through the same decode operations the
//! app's import + playback do, and report OK or the first failure.
//! `cargo run -p cutlass-engine --example import_probe -- <clip>`

use cutlass_engine::MediaEngine;

fn main() {
    let path = std::env::args().nth(1).expect("usage: import_probe <clip>");
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match probe(&path) {
        Ok(msg) => println!("OK    {name}  {msg}"),
        Err(e) => println!("FAIL  {name}  {e}"),
    }
}

fn probe(path: &str) -> anyhow::Result<String> {
    // A sound file takes the app's audio path: there is no picture to decode,
    // so import is its length and its waveform, and the waveform is the cost.
    let streams = cutlass_engine::probe_streams(path)?;
    if !streams.has_video {
        anyhow::ensure!(streams.has_audio, "no picture or sound");
        let t = std::time::Instant::now();
        let wf = cutlass_engine::audio::waveform_peaks(path, 1200);
        return Ok(format!(
            "audio dur={:.2}s wf={} in {:.1}s",
            streams.duration_s,
            wf.len(),
            t.elapsed().as_secs_f64()
        ));
    }
    let mut eng = MediaEngine::open(path)?;
    let dur = eng.duration_s();
    print!("[{}] ", eng.container());
    anyhow::ensure!(dur > 0.0, "non-positive duration");
    // keyframe sampling (import proxy build)
    let mut dims = (0u32, 0u32);
    for f in [0.0, 0.25, 0.5, 0.9] {
        let frame = eng.keyframe_at(dur * f, 480)?;
        anyhow::ensure!(frame.width > 0 && frame.height > 0, "empty keyframe at {f}");
        dims = (frame.width, frame.height);
    }
    // exact frame (settle) + streaming (playback)
    let exact = eng.frame_at(dur * 0.5, 1920)?;
    anyhow::ensure!(exact.width > 0, "empty exact frame");
    let stream = eng.stream_frame_at(dur * 0.3, 854)?;
    anyhow::ensure!(stream.width > 0, "empty stream frame");
    // waveform (import); may legitimately be empty for silent/no-audio clips
    let wf = cutlass_engine::audio::waveform_peaks(path, 200);
    Ok(format!(
        "dur={dur:.2}s proxy={}x{} exact={}x{} wf={}",
        dims.0, dims.1, exact.width, exact.height, wf.len()
    ))
}
