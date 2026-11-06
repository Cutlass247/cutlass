//! Apple Log footage through a .cube LUT, preview against export against the
//! picture that was filmed.
//!
//! `cargo run -p cutlass-core --example log_lut_check -- <outdir>`
//!
//! An iPhone 15 Pro or later records Apple Log as 10-bit ProRes with BT.2020
//! primaries and matrix and no transfer named (so it isn't taken for HDR).
//! Nothing like it had been through Cutlass. This builds that clip from a
//! known picture, colour patches over a grey ramp, using Apple's published
//! log curve, and a LUT back to Rec.709 from the same maths. The exports, on
//! V1 and on V2, and the preview, which applies the LUT itself on the GPU
//! (`renderLut` in Monitor.tsx, emulated here), are held to that LUT applied
//! at full precision; how near a 33-point LUT on log gets to the picture
//! filmed at all is printed for the record.
//!
//! What it found the first time: the LUT ran after the picture was made
//! 8-bit, which doubled its error and banded a grey ramp, and the export was
//! tagged BT.2020 primaries over Rec.709 pixels, which a colour-managed
//! player shows oversaturated.

use cutlass_core::export::{build_segments, export, ClipFx, ExportClip, ExportSettings, Overlay};
use std::io::Write;
use std::path::Path;

const W: usize = 640;
const H: usize = 360;

// Apple Log (Apple Log Profile white paper, 2023)
const R0: f64 = -0.05641088;
const RT: f64 = 0.01;
const C: f64 = 47.28711236;
const BETA: f64 = 0.00964052;
const GAMMA: f64 = 0.08550479;
const DELTA: f64 = 0.69336945;

fn log_encode(r: f64) -> f64 {
    if r < R0 {
        0.0
    } else if r < RT {
        C * (r - R0).powi(2)
    } else {
        GAMMA * (r + BETA).log2() + DELTA
    }
}

fn log_decode(p: f64) -> f64 {
    if p < C * (RT - R0).powi(2) {
        (p.max(0.0) / C).sqrt() + R0
    } else {
        2f64.powf((p - DELTA) / GAMMA) - BETA
    }
}

/// BT.709 to BT.2020 primaries, linear light.
const M709_2020: [[f64; 3]; 3] = [[0.6274, 0.3293, 0.0433], [0.0691, 0.9195, 0.0114], [0.0164, 0.0880, 0.8956]];

fn mul(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn inverse(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

/// The picture that was filmed, as Rec.709 display values: eight patches
/// across the top half, a grey ramp across the bottom. Strong colours, but
/// none with a channel at nothing: there a log LUT is close to vertical
/// (black, and the edge of the gamut), more than 33 points can follow, so
/// any LUT, Apple's own included, comes back from it off by tens of levels.
const PATCHES: [[f64; 3]; 8] = [
    [0.75, 0.75, 0.75],
    [0.75, 0.72, 0.20],
    [0.20, 0.70, 0.75],
    [0.25, 0.70, 0.25],
    [0.70, 0.25, 0.70],
    [0.75, 0.22, 0.20],
    [0.20, 0.25, 0.75],
    [0.85, 0.62, 0.50], // skin
];

fn reference() -> Vec<[u8; 3]> {
    let pw = W / PATCHES.len();
    (0..H * W)
        .map(|i| {
            let (y, x) = (i / W, i % W);
            let c = if y < H / 2 {
                PATCHES[(x / pw).min(PATCHES.len() - 1)]
            } else {
                let v = 0.02 + 0.96 * x as f64 / (W - 1) as f64;
                [v; 3]
            };
            c.map(|v| (v * 255.0).round() as u8)
        })
        .collect()
}

/// The cube: Apple Log, BT.2020 -> scene light -> BT.709 -> display.
fn cube(n: usize) -> Vec<[f64; 3]> {
    let back = inverse(&M709_2020);
    let mut out = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let p = [r, g, b].map(|v| v as f64 / (n - 1) as f64);
                let lin = mul(&back, p.map(log_decode));
                out.push(lin.map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.4)));
            }
        }
    }
    out
}

/// The preview's LUT, as Monitor.tsx's `renderLut` runs it on the 8-bit
/// frame: the table stored as 8-bit, sampled trilinearly at texel centres.
fn preview_lut(table: &[[f64; 3]], n: usize, c: [u8; 3]) -> [u8; 3] {
    let q: Vec<[f64; 3]> = table.iter().map(|t| t.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() / 255.0)).collect();
    let at = |r: usize, g: usize, b: usize| q[(b * n + g) * n + r];
    let pos = c.map(|v| v as f64 / 255.0 * (n - 1) as f64);
    let i0 = pos.map(|p| (p.floor() as usize).min(n - 2));
    let f = [0, 1, 2].map(|k| pos[k] - i0[k] as f64);
    let mut out = [0.0; 3];
    for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
        for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
            for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
                let v = at(i0[0] + dr, i0[1] + dg, i0[2] + db);
                for k in 0..3 {
                    out[k] += wr * wg * wb * v[k];
                }
            }
        }
    }
    out.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// One frame of `path` at `t` as RGB, decoded with the matrix `matrix`, or
/// with the file's own when None.
fn frame_rgb(ff: &str, path: &Path, t: f64, matrix: Option<&str>) -> anyhow::Result<Vec<[u8; 3]>> {
    let m = matrix.map(|m| format!("in_color_matrix={m}:in_range=tv:")).unwrap_or_default();
    let raw = std::process::Command::new(ff)
        .args(["-v", "error", "-ss", &format!("{t:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", &format!("scale={W}:{H}:{m}flags=bicubic,format=rgb24")])
        .args(["-f", "rawvideo", "-"])
        .output()?
        .stdout;
    anyhow::ensure!(raw.len() == W * H * 3, "decoded {} bytes from {}", raw.len(), path.display());
    Ok(raw.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect())
}

/// The file's colour tags as ffprobe reads them from its first frame.
fn tags(ff: &str, path: &Path) -> String {
    let ff = Path::new(ff);
    let name = if ff.extension().is_some() { "ffprobe.exe" } else { "ffprobe" };
    std::process::Command::new(ff.with_file_name(name))
        .args(["-v", "error", "-read_intervals", "%+#1", "-select_streams", "v:0"])
        .args(["-show_entries", "frame=color_primaries,color_transfer,color_space", "-of", "compact=p=0:nk=1"])
        .arg(path)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// Mean of `img` over each patch's middle and over the ramp's, against
/// `want`: the worst channel difference of any patch, and the ramp's mean
/// difference.
fn compare(img: &[[u8; 3]], want: &[[u8; 3]]) -> (f64, f64, usize) {
    let pw = W / PATCHES.len();
    let mean = |img: &[[u8; 3]], x0: usize, x1: usize, y0: usize, y1: usize| {
        let mut s = [0.0; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                for k in 0..3 {
                    s[k] += img[y * W + x][k] as f64;
                }
            }
        }
        s.map(|v| v / ((x1 - x0) * (y1 - y0)) as f64)
    };
    let mut worst = (0.0f64, 0usize);
    let verbose = std::env::var("LOG_LUT_VERBOSE").is_ok();
    for p in 0..PATCHES.len() {
        let (x0, x1) = (p * pw + pw / 4, (p + 1) * pw - pw / 4);
        let (a, b) = (mean(img, x0, x1, H / 8, H * 3 / 8), mean(want, x0, x1, H / 8, H * 3 / 8));
        if verbose {
            println!("    patch {p}: {:>5.1} {:>5.1} {:>5.1}  want {:>5.1} {:>5.1} {:>5.1}", a[0], a[1], a[2], b[0], b[1], b[2]);
        }
        for k in 0..3 {
            if (a[k] - b[k]).abs() > worst.0 {
                worst = ((a[k] - b[k]).abs(), p);
            }
        }
    }
    let mut ramp = 0.0;
    let (y0, y1) = (H * 5 / 8, H * 7 / 8);
    for x in (8..W - 8).step_by(8) {
        let (a, b) = (mean(img, x, x + 8, y0, y1), mean(want, x, x + 8, y0, y1));
        ramp += (0..3).map(|k| (a[k] - b[k]).abs()).sum::<f64>() / 3.0;
    }
    (worst.0, ramp / ((W - 16) / 8) as f64, worst.1)
}

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: log_lut_check <outdir>"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;
    let ff = std::env::var("FFMPEG_BINARY").unwrap_or_else(|_| "ffmpeg".into());

    // the clip, as an iPhone records it
    let reference = reference();
    let to_2020 = M709_2020;
    let frame: Vec<u8> = reference
        .iter()
        .flat_map(|px| {
            let lin = px.map(|v| (v as f64 / 255.0).powf(2.4));
            mul(&to_2020, lin).map(|v| ((log_encode(v).clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes())
        })
        .flatten()
        .collect();
    let clip = dir.join("applelog.mov");
    let mut enc = std::process::Command::new(&ff)
        .args(["-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb48le", "-s", &format!("{W}x{H}")])
        .args(["-r", "30", "-i", "-", "-vf"])
        .arg(
            "scale=out_color_matrix=bt2020:out_range=tv,format=yuv422p10le,\
             setparams=color_primaries=bt2020:color_trc=unknown:colorspace=bt2020nc:range=tv",
        )
        .args(["-c:v", "prores_ks", "-profile:v", "3"])
        .arg(&clip)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    {
        let mut stdin = enc.stdin.take().unwrap();
        for _ in 0..60 {
            stdin.write_all(&frame)?;
        }
    }
    anyhow::ensure!(enc.wait()?.success(), "could not make the Apple Log clip");
    println!("clip tagged {}", tags(&ff, &clip));

    let n = 33;
    let table = cube(n);
    let lut = dir.join("applelog_to_709.cube");
    let mut text = format!("TITLE \"Apple Log to Rec.709 (test)\"\nLUT_3D_SIZE {n}\n");
    for t in &table {
        text.push_str(&format!("{:.6} {:.6} {:.6}\n", t[0], t[1], t[2]));
    }
    std::fs::write(&lut, text)?;
    let lut_s = lut.to_string_lossy().into_owned();
    let clip_s = clip.to_string_lossy().into_owned();

    let settings = ExportSettings { width: W as u32, height: H as u32, fps: 30, ..Default::default() };
    let none = std::sync::atomic::AtomicBool::new(false);
    let ex = |start: f64, path: &str, lut: &str| ExportClip {
        start,
        len: 2.0,
        src_in: 0.0,
        path: path.to_string(),
        fx: ClipFx::default(),
        lut: lut.to_string(),
        trans_dur: 0.0,
        trans_dip: false,
        kf: Default::default(),
    };
    // on V1
    let v1 = dir.join("log_on_v1.mp4");
    export(&build_segments(vec![ex(0.0, &clip_s, &lut_s)]), &[], &[], &v1, &settings, &mut |_| {}, &none)?;
    // on V2, over V1 footage of the same size, so it covers it
    let v2 = dir.join("log_on_v2.mp4");
    let ov = Overlay {
        path: clip_s.clone(),
        src_in: 0.0,
        len: 2.0,
        start: 0.0,
        fx: ClipFx::default(),
        audio_only: false,
        lut: lut_s.clone(),
        kf: Default::default(),
    };
    export(&build_segments(vec![ex(0.0, &clip_s, "")]), &[ov], &[], &v2, &settings, &mut |_| {}, &none)?;

    // the preview: the frame as the engine converts it (the matrix it's
    // tagged with, to 8-bit RGB), then the LUT on the GPU
    let decoded = frame_rgb(&ff, &clip, 1.0, Some("bt2020"))?;
    let preview: Vec<[u8; 3]> = decoded.iter().map(|&c| preview_lut(&table, n, c)).collect();
    // the LUT at its best: on the clip's full 10 bits, as 16-bit RGB
    let ideal = {
        let raw = std::process::Command::new(&ff)
            .args(["-v", "error", "-ss", "1.000", "-i"])
            .arg(&clip)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!(
                "scale=in_color_matrix=bt2020:in_range=tv,format=rgb48le,lut3d=file='{}',format=rgb24",
                lut_s.replace('\\', "/").replace(':', "\\:")
            ))
            .args(["-f", "rawvideo", "-"])
            .output()?
            .stdout;
        anyhow::ensure!(raw.len() == W * H * 3, "ideal LUT render gave {} bytes", raw.len());
        raw.chunks_exact(3).map(|p| [p[0], p[1], p[2]]).collect::<Vec<[u8; 3]>>()
    };

    // The preview gets more room than the export: it applies the LUT to an
    // 8-bit frame (the picture element it's drawn from can hold no more),
    // and one 8-bit step of log comes out of the LUT as about three. Its
    // interpolation isn't what costs it: tetrahedral, as the export's, was
    // tried in the shader and measured the same.
    let check = |what: &str, img: &[[u8; 3]], against: &[[u8; 3]], name: &str| {
        let (patch, ramp, which) = compare(img, against);
        // the export measured 3.9; with the LUT after the 8-bit step, 5.9
        let (p_max, r_max) = if what == "preview" || name == "the preview" { (8.0, 4.0) } else { (5.0, 3.0) };
        let pass = patch < p_max && ramp < r_max;
        println!(
            "{} {what} vs {name}: patches within {patch:.1} levels (worst patch {which}), ramp {ramp:.1} on average",
            if pass { "PASS" } else { "FAIL" }
        );
        pass
    };
    // how close a 33-point LUT on log can get at all, for the record
    let (p, r, _) = compare(&ideal, &reference);
    println!("  (the LUT itself, at full precision, against the picture filmed: patches within {p:.1}, ramp {r:.1})");
    let mut ok = check("preview", &preview, &ideal, "the LUT at full precision");
    for (name, out) in [("V1", &v1), ("V2", &v2)] {
        // as a player reads it: by the file's own tags
        let img = frame_rgb(&ff, out, 1.0, None)?;
        ok &= check(&format!("{name} export"), &img, &ideal, "the LUT at full precision");
        ok &= check(&format!("{name} export"), &img, &preview, "the preview");
        let t = tags(&ff, out);
        let tagged = t.starts_with("bt709|") && t.contains("|bt709");
        ok &= tagged;
        println!("{} {name} export tagged {t}", if tagged { "PASS" } else { "FAIL" });
    }
    anyhow::ensure!(ok, "see the FAIL lines above");
    println!("OK");
    Ok(())
}
