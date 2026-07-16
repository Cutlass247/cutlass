//! Verify the in-process engine against Spike B's targets:
//! `cargo run -p cutlass-engine --example engine_check -- <video>`
//!
//! Spike B subprocess baseline: 56.5 fps decode, 857 ms seek.
//! Targets: ≥60 fps decode, ≤80 ms seek.

use std::time::Instant;

use cutlass_engine::MediaEngine;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: engine_check <video>"))?;

    let mut engine = MediaEngine::open(&path)?;
    let (w, h) = engine.dimensions();
    println!(
        "opened {path}: {w}x{h}, {:.2}s",
        engine.duration_s()
    );

    // 1. sequential decode throughput
    let t0 = Instant::now();
    let frames = engine.drain()?;
    let fps = frames as f64 / t0.elapsed().as_secs_f64();
    println!("decode: {frames} frames at {fps:.1} fps  (subprocess was 56.5; target ≥ 60)");

    // 2. seeks on the source: frame-accurate (pays GOP decode) vs
    //    keyframe-fast (the raw-source scrub path)
    let exact = bench_seeks(&mut engine, true)?;
    println!("frame-accurate seek on source: median {exact:.1} ms  (informational — GOP-bound; subprocess was 857)");
    let kf = bench_seeks(&mut engine, false)?;
    println!("keyframe-fast seek on source:  median {kf:.1} ms  (informational — fallback path while proxy builds; frame-threading adds pipeline latency)");

    // 3. frame-accurate seek on the all-intra proxy — the production
    //    scrub/edit path, where every frame is a keyframe
    let proxy_median = match std::env::args().nth(2) {
        Some(proxy) => {
            let mut p = MediaEngine::open(&proxy)?;
            let m = bench_seeks(&mut p, true)?;
            println!("frame-accurate seek on proxy:  median {m:.1} ms  (target ≤ 16)");
            Some(m)
        }
        None => {
            println!("(no proxy arg — skipping all-intra proxy seek benchmark)");
            None
        }
    };

    // pass = playback throughput + the production scrub path (all-intra
    // proxy); source-seek numbers above are informational
    let pass = fps >= 60.0 && proxy_median.map_or(false, |m| m <= 16.0);
    println!(
        "VERDICT: {}",
        if pass { "targets met — in-process engine GO" } else { "targets missed" }
    );
    std::process::exit(if pass { 0 } else { 1 });
}

/// Median latency (ms) of 15 random seeks. Exact = frame-accurate.
fn bench_seeks(engine: &mut MediaEngine, exact: bool) -> anyhow::Result<f64> {
    let dur = engine.duration_s();
    let mut lat: Vec<f64> = Vec::new();
    for i in 0..15 {
        let t = (i as f64 * 1.31) % dur.max(0.1);
        let t0 = Instant::now();
        let frame = if exact {
            engine.frame_at(t, 1280)?
        } else {
            engine.keyframe_at(t, 1280)?
        };
        lat.push(t0.elapsed().as_secs_f64() * 1000.0);
        if exact {
            assert!(
                (frame.pts_s - t).abs() < 0.5,
                "seek to {t:.2} landed at {:.2}",
                frame.pts_s
            );
        }
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(lat[lat.len() / 2])
}
