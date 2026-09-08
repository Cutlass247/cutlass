//! Regression: a hole between two clips too short to hold a single frame used
//! to hang the export outright. build_segments must not turn one into a gap.
//! `cargo run -p cutlass-core --example gap_regression`

use cutlass_core::export::{build_segments, ExportClip, Segment};

fn clip(start: f64, len: f64) -> ExportClip {
    ExportClip {
        start,
        len,
        src_in: 0.0,
        path: "C:/clips/a.mp4".into(),
        fx: Default::default(),
        kf: Default::default(),
        lut: String::new(),
        trans_dur: 0.0,
        trans_dip: false,
    }
}

fn gaps(segs: &[Segment]) -> Vec<f64> {
    segs.iter()
        .filter_map(|s| match s {
            Segment::Gap { len } => Some(*len),
            _ => None,
        })
        .collect()
}

fn main() {
    // The real case: 223.10 + 46.3576 lands at 269.4576 against a next clip
    // starting at 269.46 -- a 2.4 ms hole nobody placed, which stalled a
    // 75-minute export dead at 6%.
    let drift = vec![clip(223.10, 46.3576), clip(269.46, 1.79)];
    let segs = build_segments(drift);
    // The leading 223.1s hole before the first clip is a real gap and belongs
    // there; only a sub-frame one between the clips is the bug.
    let tiny: Vec<f64> = gaps(&segs).into_iter().filter(|g| *g < 1.0).collect();
    println!(
        "  floating-point drift (2.4ms): sub-second gaps={:?}  -> {}",
        tiny,
        if tiny.is_empty() { "PASS (absorbed)" } else { "FAIL (would hang)" }
    );

    // A gap somebody actually meant must still be there.
    let real = vec![clip(0.0, 5.0), clip(9.0, 5.0)];
    let g = gaps(&build_segments(real));
    println!(
        "  deliberate 4s gap:            gaps={:?}  -> {}",
        g,
        if g.len() == 1 && (g[0] - 4.0).abs() < 1e-6 { "PASS (kept)" } else { "FAIL" }
    );

    // Nothing between 1ms and a frame may survive as a gap.
    let mut worst = None;
    for ms in 1..=49 {
        let d = ms as f64 / 1000.0;
        let g = gaps(&build_segments(vec![clip(0.0, 5.0), clip(5.0 + d, 5.0)]));
        if !g.is_empty() {
            worst = Some(ms);
            break;
        }
    }
    println!(
        "  sweep 1..49ms:                first surviving gap={:?} -> {}",
        worst,
        if worst.is_none() { "PASS (none survive)" } else { "FAIL" }
    );
}
