//! Playback sounds like the export: a clip's fades and volume keyframes,
//! heard through the player, against the same clip exported, 50ms by 50ms.
//!
//! `cargo run -p cutlass-engine --example playback_gain_check -- <outdir>`
//!
//! Both used to be heard only in the export: a music bed faded out, or dipped
//! under a voice with keyframes, played at full volume in the player.

use cutlass_core::export::{export, ClipFx, ExportFormat, ExportSettings, Overlay, Segment};
use cutlass_engine::player::{AudioClip, TrackReader};

const RATE: u32 = 48_000;

fn rms(s: &[f64]) -> f64 {
    (s.iter().map(|v| v * v).sum::<f64>() / s.len().max(1) as f64).sqrt()
}

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: playback_gain_check <outdir>"));
    std::fs::create_dir_all(&dir)?;
    let ff = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());
    let tone = dir.join("tone.wav");
    let made = std::process::Command::new(&ff)
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=f=440:d=8:sample_rate=48000", "-ac", "2"])
        .arg(&tone)
        .status()?;
    anyhow::ensure!(made.success(), "could not make the tone");
    let tone_s = tone.to_string_lossy().into_owned();

    // 4s from 1s in: faded in over 0.5s and out over 1s, its volume dipped
    // from full to 0.3 between 1.5s and 2s
    let (len, fade_in, fade_out) = (4.0, 0.5, 1.0);
    let kf = vec![(1.5, 1.0), (2.0, 0.3)];

    let mut reader = TrackReader::new(
        vec![AudioClip {
            path: tone_s.clone(),
            start: 0.0,
            len,
            src_in: 1.0,
            volume: 1.0,
            speed: 1.0,
            audio_offset: 0.0,
            fade_in,
            fade_out,
            volume_kf: kf.clone(),
        }],
        RATE,
        0.0,
    );
    let played: Vec<f64> = reader
        .read((len * RATE as f64) as usize)
        .chunks_exact(2)
        .map(|p| (p[0] as f64 + p[1] as f64) / 2.0)
        .collect();

    // the same clip as a bed under 4s of nothing, exported with its sound
    // uncompressed (ProRes's PCM), so nothing but the gain differs
    let out = dir.join("bed.mov");
    let bed = Overlay {
        path: tone_s,
        src_in: 1.0,
        len,
        start: 0.0,
        fx: ClipFx { fade_in, fade_out, ..Default::default() },
        audio_only: true,
        lut: String::new(),
        kf: [("volume".to_string(), kf)].into_iter().collect(),
    };
    let settings = ExportSettings { width: 320, height: 180, fps: 30, format: ExportFormat::MovProres, ..Default::default() };
    export(&[Segment::Gap { len }], &[bed], &[], &out, &settings, &mut |_| {}, &std::sync::atomic::AtomicBool::new(false))?;
    let pcm = std::process::Command::new(&ff)
        .args(["-v", "error", "-i"])
        .arg(&out)
        .args(["-vn", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"])
        .output()?
        .stdout;
    let exported: Vec<f64> = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f64 / 32768.0).collect();

    // loudness in 50ms windows, as a share of the tone at full
    let win = (RATE / 20) as usize;
    let full = rms(&played[(1.0 * RATE as f64) as usize..][..win]); // 1.0s: faded in, not yet dipped
    let mut worst = (0.0f64, 0.0f64);
    for i in 0..(len * 20.0) as usize - 1 {
        let (a, b) = (&played[i * win..(i + 1) * win], exported.get(i * win..(i + 1) * win).unwrap_or(&[]));
        let d = (rms(a) - rms(b)).abs() / full;
        if d > worst.0 {
            worst = (d, i as f64 * 0.05);
        }
    }
    let ok = worst.0 < 0.06;
    println!(
        "{} playback's fades and volume keyframes match the export: furthest {:.0}% of full apart, at {:.2}s",
        if ok { "PASS" } else { "FAIL" },
        worst.0 * 100.0,
        worst.1
    );
    for t in [0.25, 1.0, 1.75, 2.5, 3.75] {
        let i = (t * 20.0) as usize;
        println!(
            "  at {t:.2}s: played {:.2}, exported {:.2} of full",
            rms(&played[i * win..(i + 1) * win]) / full,
            rms(&exported[i * win..(i + 1) * win]) / full
        );
    }
    anyhow::ensure!(ok, "see the FAIL line above");
    println!("OK");
    Ok(())
}
