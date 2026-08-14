//! Template-matching motion tracker for the censor box. Given a starting
//! region, it follows that patch across the clip and returns position
//! keyframes (clip-relative time, normalised centre x/y) that the existing
//! keyframe machinery uses to make the box follow the subject.
//!
//! Fully offline, no ML: it grabs the patch inside the box on the first
//! frame, then for each later frame searches a small window for the best
//! match using zero-mean SAD (mean-subtracted so it tolerates brightness
//! changes), with integral-image patch means and per-row early-out to stay
//! fast. Best for a distinct patch (face, plate, logo) with smooth motion.

use anyhow::Context;
use ffmpeg_sidecar::command::FfmpegCommand;

// Tracking runs on small grayscale frames. Coords are normalised, so the
// fixed (aspect-squashing) size doesn't affect correctness — a point at
// fractional (x,y) maps the same in the squashed frame and the real one.
// Small frames keep decode + matching cheap. Coords are normalised so the
// aspect squash doesn't affect correctness.
const TW: usize = 256;
const TH: usize = 144;

/// A `tw`×`th` patch of `frame` at (x,y), mean-subtracted to `f32`.
fn zero_mean_patch(frame: &[u8], stride: usize, x: usize, y: usize, tw: usize, th: usize) -> Vec<f32> {
    let mut sum = 0u64;
    for j in 0..th {
        let row = (y + j) * stride + x;
        for i in 0..tw {
            sum += frame[row + i] as u64;
        }
    }
    let mean = sum as f32 / (tw * th) as f32;
    let mut out = Vec::with_capacity(tw * th);
    for j in 0..th {
        let row = (y + j) * stride + x;
        for i in 0..tw {
            out.push(frame[row + i] as f32 - mean);
        }
    }
    out
}

/// (w+1)×(h+1) integral image so any patch sum is an O(1) lookup.
fn integral(frame: &[u8], w: usize, h: usize) -> Vec<u64> {
    let mut ii = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row_sum = 0u64;
        for x in 0..w {
            row_sum += frame[y * w + x] as u64;
            ii[(y + 1) * (w + 1) + (x + 1)] = ii[y * (w + 1) + (x + 1)] + row_sum;
        }
    }
    ii
}

#[inline]
fn patch_sum(ii: &[u64], w: usize, x: usize, y: usize, tw: usize, th: usize) -> u64 {
    let s = w + 1;
    ii[(y + th) * s + (x + tw)] + ii[y * s + x] - ii[y * s + (x + tw)] - ii[(y + th) * s + x]
}

/// Zero-mean SAD of the template against `frame` at (x,y), given the
/// candidate patch mean. Returns early (as +inf) once it exceeds `best`.
#[inline]
fn zsad(frame: &[u8], stride: usize, x: usize, y: usize, tmpl: &[f32], tw: usize, th: usize, pmean: f32, best: f64) -> f64 {
    let mut s = 0f64;
    for j in 0..th {
        let row = (y + j) * stride + x;
        let trow = j * tw;
        for i in 0..tw {
            let fv = frame[row + i] as f32 - pmean;
            s += (fv - tmpl[trow + i]).abs() as f64;
        }
        if s >= best {
            return f64::INFINITY;
        }
    }
    s
}

/// Track the box across `frames` (each `w`×`h` gray). Box is the normalised
/// centre (cx,cy) + size (bw,bh) *as placed at `anchor_frame`* — the template
/// is grabbed there and followed both forward and backward, so the box hugs
/// that exact spot over the whole clip. Returns (time_s, cx, cy) per frame.
pub fn track_frames(
    frames: &[Vec<u8>],
    w: usize,
    h: usize,
    cx: f64,
    cy: f64,
    bw: f64,
    bh: f64,
    fps: f64,
    anchor_frame: usize,
    progress: &mut dyn FnMut(f32),
) -> Vec<(f64, f64, f64)> {
    if frames.is_empty() {
        return vec![(0.0, cx, cy)];
    }
    let n = frames.len();
    let anchor = anchor_frame.min(n - 1);
    // template size in px, capped for speed
    let tw = (((bw * w as f64).round() as isize).clamp(8, 90) as usize).min(w);
    let th = (((bh * h as f64).round() as isize).clamp(8, 90) as usize).min(h);
    // placed top-left, clamped inside the frame
    let clampi = |v: isize, hi: isize| v.max(0).min(hi.max(0));
    let ax = clampi((cx * w as f64) as isize - tw as isize / 2, w as isize - tw as isize) as usize;
    let ay = clampi((cy * h as f64) as isize - th as isize / 2, h as isize - th as isize) as usize;

    // reference patch from the frame where the user placed the box
    let tmpl = zero_mean_patch(&frames[anchor], w, ax, ay, tw, th);
    let r = ((tw.max(th) as f64) * 0.5).round().clamp(8.0, 26.0) as isize;
    let inv_area = 1.0 / (tw * th) as f32;

    // best-matching top-left in `frame`, searched near (px,py)
    let search = |frame: &[u8], px: usize, py: usize| -> (usize, usize) {
        let ii = integral(frame, w, h);
        let x0 = (px as isize - r).max(0) as usize;
        let x1 = ((px as isize + r).min(w as isize - tw as isize)).max(0) as usize;
        let y0 = (py as isize - r).max(0) as usize;
        let y1 = ((py as isize + r).min(h as isize - th as isize)).max(0) as usize;
        let mut best = f64::INFINITY;
        let (mut bx, mut by) = (px, py);
        for yy in y0..=y1 {
            for xx in x0..=x1 {
                let pmean = patch_sum(&ii, w, xx, yy, tw, th) as f32 * inv_area;
                let s = zsad(frame, w, xx, yy, &tmpl, tw, th, pmean, best);
                if s < best {
                    best = s;
                    bx = xx;
                    by = yy;
                }
            }
        }
        (bx, by)
    };

    let mut pos = vec![(ax, ay); n];
    // forward from the anchor
    let (mut px, mut py) = (ax, ay);
    for i in anchor + 1..n {
        let (bx, by) = search(&frames[i], px, py);
        pos[i] = (bx, by);
        px = bx;
        py = by;
        if i % 6 == 0 {
            progress(i as f32 / n as f32);
        }
    }
    // backward from the anchor
    let (mut px, mut py) = (ax, ay);
    for i in (0..anchor).rev() {
        let (bx, by) = search(&frames[i], px, py);
        pos[i] = (bx, by);
        px = bx;
        py = by;
    }
    (0..n)
        .map(|i| {
            let (px, py) = pos[i];
            (i as f64 / fps, (px + tw / 2) as f64 / w as f64, (py + th / 2) as f64 / h as f64)
        })
        .collect()
}

/// Extract the clip's [src_in, src_in+len] as small gray frames at `fps`.
fn extract_gray(path: &str, src_in: f64, len: f64, fps: f64) -> anyhow::Result<Vec<Vec<u8>>> {
    let tmp = std::env::temp_dir().join(format!("cutlass_track_{}.gray", std::process::id()));
    let vf = format!("fps={fps:.3},scale={TW}:{TH},format=gray");
    let mut child = FfmpegCommand::new()
        .args(["-ss", &format!("{src_in}")])
        .input(path)
        .args([
            "-t", &format!("{len}"),
            "-an",
            "-vf", &vf,
            "-f", "rawvideo",
            "-pix_fmt", "gray",
            "-y",
            tmp.to_str().context("temp path")?,
        ])
        .spawn()?;
    // drain events so stderr never blocks the writer
    if let Ok(iter) = child.iter() {
        for _ in iter {}
    }
    let _ = child.as_inner_mut().wait();

    let bytes = std::fs::read(&tmp).context("read extracted frames")?;
    let _ = std::fs::remove_file(&tmp);
    let fsize = TW * TH;
    anyhow::ensure!(fsize > 0 && !bytes.is_empty(), "no frames extracted");
    Ok(bytes.chunks_exact(fsize).map(|c| c.to_vec()).collect())
}

/// Track a censor region across a clip's source. Returns position keyframes
/// (clip-relative time, normalised centre x/y). `progress` gets 0..=1.
pub fn track_region(
    path: &str,
    src_in: f64,
    len: f64,
    cx: f64,
    cy: f64,
    bw: f64,
    bh: f64,
    anchor: f64,
    progress: &mut dyn FnMut(f32),
) -> anyhow::Result<Vec<(f64, f64, f64)>> {
    progress(0.02);
    // cap the frame count (~≤250) so long clips stay responsive; the box
    // interpolates smoothly between keyframes anyway
    let fps = (250.0 / len.max(1.0)).clamp(2.0, 10.0);
    let frames = extract_gray(path, src_in, len, fps)?;
    progress(0.08);
    // the box was placed at `anchor` (clip-relative seconds) → that frame
    let anchor_frame = ((anchor.max(0.0) * fps).round() as usize).min(frames.len().saturating_sub(1));
    let raw = track_frames(&frames, TW, TH, cx, cy, bw, bh, fps, anchor_frame, &mut |f| progress(0.08 + 0.9 * f));
    progress(1.0);
    Ok(decimate(&raw))
}

/// Thin the per-frame track into keyframes: keep the ends and any point that
/// moved meaningfully, so a mostly-still box writes only a handful.
fn decimate(raw: &[(f64, f64, f64)]) -> Vec<(f64, f64, f64)> {
    if raw.len() <= 2 {
        return raw.to_vec();
    }
    let mut out = vec![raw[0]];
    let (mut lx, mut ly) = (raw[0].1, raw[0].2);
    for p in &raw[1..raw.len() - 1] {
        if (p.1 - lx).abs() > 0.006 || (p.2 - ly).abs() > 0.006 {
            out.push(*p);
            lx = p.1;
            ly = p.2;
        }
    }
    out.push(*raw.last().unwrap());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // a w×h frame with a bright square at (sx,sy) of size `sq` on a dark field
    fn frame_with_square(w: usize, h: usize, sx: isize, sy: isize, sq: usize) -> Vec<u8> {
        let mut f = vec![40u8; w * h];
        for j in 0..sq as isize {
            for i in 0..sq as isize {
                let x = sx + i;
                let y = sy + j;
                if x >= 0 && (x as usize) < w && y >= 0 && (y as usize) < h {
                    // slight gradient so the patch has structure to lock onto
                    f[y as usize * w + x as usize] = (200 + (i + j) % 40) as u8;
                }
            }
        }
        f
    }

    #[test]
    fn tracks_a_moving_square() {
        let (w, h) = (320usize, 180usize);
        let sq = 40usize;
        // square moves +7x,+4y per frame starting near the centre-left
        let start = (90isize, 60isize);
        let (vx, vy) = (7isize, 4isize);
        let n = 12;
        let frames: Vec<Vec<u8>> = (0..n)
            .map(|i| frame_with_square(w, h, start.0 + vx * i, start.1 + vy * i, sq))
            .collect();
        // initial box centred on the square in frame 0
        let cx0 = (start.0 as f64 + sq as f64 / 2.0) / w as f64;
        let cy0 = (start.1 as f64 + sq as f64 / 2.0) / h as f64;
        let bw = sq as f64 / w as f64;
        let bh = sq as f64 / h as f64;
        let kf = track_frames(&frames, w, h, cx0, cy0, bw, bh, 10.0, 0, &mut |_| {});
        assert_eq!(kf.len(), n as usize);
        // by the last frame the tracked centre should be near the square's
        let last = kf.last().unwrap();
        let exp_cx = (start.0 as f64 + vx as f64 * (n - 1) as f64 + sq as f64 / 2.0) / w as f64;
        let exp_cy = (start.1 as f64 + vy as f64 * (n - 1) as f64 + sq as f64 / 2.0) / h as f64;
        let dx = (last.1 - exp_cx).abs() * w as f64;
        let dy = (last.2 - exp_cy).abs() * h as f64;
        assert!(dx < 6.0 && dy < 6.0, "tracked ({:.3},{:.3}) vs expected ({:.3},{:.3}); err px ({dx:.1},{dy:.1})", last.1, last.2, exp_cx, exp_cy);
    }

    #[test]
    fn tracks_bidirectionally_from_mid_anchor() {
        let (w, h) = (320usize, 180usize);
        let sq = 40usize;
        let start = 40isize;
        let vx = 8isize;
        let n = 16i64;
        let frames: Vec<Vec<u8>> = (0..n)
            .map(|i| frame_with_square(w, h, start + vx * i as isize, 60, sq))
            .collect();
        let mid = (n / 2) as usize;
        // box placed on the square *at the middle frame*
        let sqx_mid = start + vx * mid as isize;
        let cx = (sqx_mid as f64 + sq as f64 / 2.0) / w as f64;
        let cy = (60.0 + sq as f64 / 2.0) / h as f64;
        let (bw, bh) = (sq as f64 / w as f64, sq as f64 / h as f64);
        let kf = track_frames(&frames, w, h, cx, cy, bw, bh, 10.0, mid, &mut |_| {});
        assert_eq!(kf.len(), n as usize);
        // frame 0 (backward) should sit on the square's start, last (forward) on its end
        let exp_first = (start as f64 + sq as f64 / 2.0) / w as f64;
        let exp_last = ((start + vx * (n as isize - 1)) as f64 + sq as f64 / 2.0) / w as f64;
        let ef = (kf.first().unwrap().1 - exp_first).abs() * w as f64;
        let el = (kf.last().unwrap().1 - exp_last).abs() * w as f64;
        assert!(ef < 6.0, "backward track off by {ef:.1}px");
        assert!(el < 6.0, "forward track off by {el:.1}px");
    }

    // Full pipeline: ffmpeg-extract a generated clip, then track. Needs a real
    // ffmpeg, so it self-skips unless FFMPEG_BINARY is set (matching how the
    // app pins the bundled binary). Run: FFMPEG_BINARY=… cargo test track_real
    #[test]
    fn track_real_video_pipeline() {
        let Ok(ff) = std::env::var("FFMPEG_BINARY") else {
            eprintln!("skipping: FFMPEG_BINARY not set");
            return;
        };
        let vid = std::env::temp_dir().join("cutlass_track_it.mp4");
        // a *textured* 80×80 patch (testsrc2) sliding left→right over a gray
        // field for 3s — texture is what template matching locks onto
        let status = std::process::Command::new(&ff)
            .args([
                "-y",
                "-f", "lavfi", "-i", "color=c=gray:s=640x360:r=15:d=3",
                "-f", "lavfi", "-i", "testsrc2=s=80x80:r=15",
                "-filter_complex", "[0:v][1:v]overlay=x='40+150*t':y=140,format=yuv420p",
                "-t", "3",
                "-c:v", "libopenh264",
                vid.to_str().unwrap(),
            ])
            .status()
            .expect("run ffmpeg");
        assert!(status.success(), "ffmpeg failed to generate the test clip");

        // start the box centred on the square in the first frame (~x=80,y=180)
        let kf = track_region(vid.to_str().unwrap(), 0.0, 3.0, 80.0 / 640.0, 0.5, 80.0 / 640.0, 80.0 / 360.0, 0.0, &mut |_| {})
            .expect("track");
        let _ = std::fs::remove_file(&vid);
        assert!(kf.len() > 10, "got {} keyframes", kf.len());
        let first = kf.first().unwrap();
        let last = kf.last().unwrap();
        // the box should have followed the square well to the right, staying centred vertically
        assert!(last.1 - first.1 > 0.35, "x should increase; {:.3} -> {:.3}", first.1, last.1);
        assert!((last.2 - 0.5).abs() < 0.12, "y should stay centred, got {:.3}", last.2);
    }
}
