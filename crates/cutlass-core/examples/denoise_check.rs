//! Audit harness for voice cleanup: export real speech through the real
//! export path with the denoiser off and at each strength, so the damage it
//! does can be measured rather than guessed at.
//!
//! `cargo run -p cutlass-core --example denoise_check -- <speech-clip> <outdir>`
//!
//! Why this exists: the denoiser shipped with a noise floor of `nf=-25`, four
//! notches from the most destructive value the filter accepts, and took 11–12
//! dB of real voice out along with the hiss — which is why exports came back
//! muffled. A steady sine probe showed nothing, because a denoiser treats a
//! steady tone as signal; only broadband speech reveals it. So this measures
//! real speech, through `export()`, exactly as a user's render would.

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};

fn settings() -> ExportSettings {
    ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() }
}

/// Mean volume of one octave-ish band, in dBFS. A bandpass then a level
/// reading: comparing the same band across two renders is what shows whether
/// the denoiser is eating the voice.
fn band_db(path: &std::path::Path, hz: u32) -> Option<f64> {
    let out = std::process::Command::new(
        std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into()),
    )
    .args([
        "-hide_banner",
        "-nostats",
        "-i",
        &path.to_string_lossy(),
        "-af",
        &format!("bandpass=f={hz}:width_type=o:w=0.5,volumedetect"),
        "-f",
        "null",
        "-",
    ])
    .output()
    .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find(|l| l.contains("mean_volume:"))
        .and_then(|l| l.split("mean_volume:").nth(1))
        .and_then(|r| r.trim().split(' ').next())
        .and_then(|v| v.parse::<f64>().ok())
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let src = a.next().expect("usage: denoise_check <speech-clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    let render = |label: &str, fx: ClipFx| -> anyhow::Result<std::path::PathBuf> {
        let segs = vec![Segment::Clip {
            path: src.clone(),
            src_in: 0.0,
            len: 20.0,
            fx,
            lut: String::new(),
        }];
        let out = dir.join(format!("{label}.mp4"));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        export(&segs, &[], &[], &out, &settings(), &mut |_| {}, &cancel)?;
        Ok(out)
    };

    println!("rendering through the real export path...");
    let baseline = render("00-off", ClipFx::default())?;

    // The bands that matter for speech. 5k and 9k are where the old setting
    // did its damage: consonants and air, the difference between "clear" and
    // "muffled".
    let bands = [500u32, 2000, 5000, 9000];
    let base: Vec<Option<f64>> = bands.iter().map(|b| band_db(&baseline, *b)).collect();

    println!();
    println!("change vs. cleanup OFF, in dB (negative = removed):");
    println!("  {:<22} {:>8} {:>8} {:>8} {:>8}", "", "500 Hz", "2 kHz", "5 kHz", "9 kHz");

    for (label, strength) in
        [("Light (0.0)", 0.0), ("default (0.5)", 0.5), ("Normal (0.6)", 0.6), ("Strong (1.0)", 1.0)]
    {
        let fx = ClipFx { denoise: 1.0, denoise_strength: strength, ..Default::default() };
        let out = render(&format!("s{:.2}", strength), fx)?;
        print!("  {label:<22}");
        for (i, b) in bands.iter().enumerate() {
            match (band_db(&out, *b), base[i]) {
                (Some(v), Some(b0)) => print!("{:>8.1}", v - b0),
                _ => print!("{:>8}", "?"),
            }
        }
        println!();
    }

    println!();
    println!("For reference, the bug this replaced took 11-12 dB out at 5-9 kHz.");
    println!("Anything beyond about -3 dB at 9 kHz on the DEFAULT is a regression.");
    println!("Renders left in {} to listen to.", dir.display());
    Ok(())
}
