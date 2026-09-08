//! Audit harness: the things that go wrong on a real machine — media on a
//! drive that got unplugged, a truncated file, an output that can't be written.
//! What matters is whether the message tells the user what to do.
//! `cargo run -p cutlass-core --example failure_modes -- <good clip> <outdir>`

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment};

fn seg(path: &str, len: f64) -> Segment {
    Segment::Clip {
        path: path.to_string(),
        src_in: 0.0,
        len,
        fx: ClipFx::default(),
        lut: String::new(),
    }
}

fn try_export(name: &str, segs: &[Segment], out: &std::path::Path) {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let settings = ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() };
    let t = std::time::Instant::now();
    match export(segs, &[], &[], out, &settings, &mut |_| {}, &cancel) {
        Ok(_) => println!("  {name}: exported ok in {:.0}s", t.elapsed().as_secs_f64()),
        Err(e) => {
            let msg = format!("{e:#}");
            let first = msg.lines().next().unwrap_or("").trim();
            println!("  {name}: failed in {:.0}s\n      {}", t.elapsed().as_secs_f64(), &first[..first.len().min(150)]);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let good = a.next().expect("usage: failure_modes <clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    // 1. the drive got unplugged, or the file was moved
    println!("== media that isn't there ==");
    try_export(
        "missing file",
        &[seg(&good, 1.0), seg("D:/nowhere/gone forever.mp4", 1.0)],
        &dir.join("missing.mp4"),
    );

    // 2. a file that exists but has nothing usable in it
    println!("== unreadable media ==");
    let empty = dir.join("empty.mp4");
    std::fs::write(&empty, b"")?;
    try_export("zero-byte file", &[seg(&good, 1.0), seg(&empty.to_string_lossy(), 1.0)], &dir.join("zero.mp4"));

    let truncated = dir.join("truncated.mp4");
    let bytes = std::fs::read(&good)?;
    std::fs::write(&truncated, &bytes[..bytes.len() / 20])?;
    try_export("truncated file", &[seg(&good, 1.0), seg(&truncated.to_string_lossy(), 1.0)], &dir.join("trunc.mp4"));

    // 3. somewhere the output cannot go
    println!("== output that can't be written ==");
    try_export("output in a missing folder", &[seg(&good, 1.0)], &dir.join("no such folder/out.mp4"));

    Ok(())
}
