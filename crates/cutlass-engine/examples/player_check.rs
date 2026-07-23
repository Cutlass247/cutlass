//! End-to-end playback check on the real audio device: play a 2-clip
//! timeline with a gap, verify the clock advances at wall speed and
//! crosses the silent gap. Audible for ~3s.
//! `cargo run -p cutlass-engine --example player_check -- <video-with-audio>`

use std::time::{Duration, Instant};

use cutlass_engine::player::{start, AudioClip};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: player_check <video>");
    let clips = vec![
        AudioClip { path: path.clone(), start: 0.0, len: 1.0, src_in: 0.0, volume: 1.0, speed: 1.0 },
        // 0.5 s gap → silence
        AudioClip { path: path.clone(), start: 1.5, len: 1.0, src_in: 5.0, volume: 1.0, speed: 1.0 },
    ];

    let handle = start(vec![clips], 0.2)?;
    let wall = Instant::now();
    let c0 = handle.clock();
    std::thread::sleep(Duration::from_millis(1200));
    let c1 = handle.clock();
    let drift = (c1 - c0) - wall.elapsed().as_secs_f64();
    println!("clock: {c0:.3} -> {c1:.3} (wall drift {:.0} ms)", drift * 1000.0);
    assert!(c1 > c0 + 0.9, "clock barely advanced: {c0:.3} -> {c1:.3}");
    assert!(drift.abs() < 0.25, "clock drifting vs wall time: {drift:.3}s");

    // let it finish (through the gap and clip 2) and confirm it ends
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.ended() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let end_t = handle.stop();
    println!("ended={} at t={end_t:.3}", handle.ended());
    assert!(handle.ended(), "playback never ended");
    assert!(end_t >= 2.4, "ended too early: {end_t:.3}");
    println!("OK");
    Ok(())
}
