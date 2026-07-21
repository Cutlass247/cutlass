//! Verify streaming playback is real-time capable: request frames at
//! 30 fps across the clip and confirm total decode+scale time leaves
//! comfortable headroom (frames come faster than wall time needs them).
//! `cargo run -p cutlass-engine --example stream_check -- <video>`

use std::time::Instant;

use cutlass_engine::MediaEngine;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: stream_check <video>");
    let mut eng = MediaEngine::open(&path)?;
    let dur = eng.duration_s().min(10.0);
    let fps = 30.0;
    let n = (dur * fps) as usize;

    let t0 = Instant::now();
    let mut frames = 0;
    let mut last_pts = -1.0;
    let mut monotonic = true;
    for i in 0..n {
        let t = i as f64 / fps;
        let f = eng.stream_frame_at(t, 854)?; // 480p-ish playback res
        if f.pts_s + 1e-6 < last_pts {
            monotonic = false;
        }
        last_pts = f.pts_s;
        frames += 1;
    }
    let secs = t0.elapsed().as_secs_f64();
    let played = n as f64 / fps;
    let realtime_margin = played / secs;
    println!(
        "streamed {frames} frames ({played:.1}s of video) in {secs:.2}s → {realtime_margin:.1}× real-time"
    );
    assert!(monotonic, "frame pts went backwards during forward stream");
    assert!(
        realtime_margin > 1.2,
        "not real-time capable ({realtime_margin:.2}× — needs ≥1× plus headroom)"
    );

    // a backward jump must re-seek and still deliver a frame
    let back = eng.stream_frame_at(0.5, 854)?;
    println!("after backward jump to 0.5s → frame pts {:.2}s", back.pts_s);
    assert!(back.pts_s < 1.5, "backward jump did not re-seek");

    println!("OK");
    Ok(())
}
