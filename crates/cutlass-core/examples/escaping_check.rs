//! Audit harness: characters users really type and really name files with.
//! `cargo run -p cutlass-core --example escaping_check -- <clip> <outdir>`

use cutlass_core::export::{export, ClipFx, ExportSettings, Segment, Title};

fn settings() -> ExportSettings {
    ExportSettings { width: 640, height: 360, fps: 30, ..Default::default() }
}

fn title(text: &str) -> Title {
    Title {
        text: text.to_string(),
        start: 0.0,
        len: 1.0,
        pos_x: 0.0,
        pos_y: 0.0,
        font_size: 48.0,
        bg: 0.0,
    }
}

fn one(src: &str) -> Vec<Segment> {
    vec![Segment::Clip {
        path: src.to_string(),
        src_in: 0.0,
        len: 1.0,
        fx: ClipFx::default(),
        lut: String::new(),
    }]
}

fn run(name: &str, segs: &[Segment], titles: &[Title], out: &std::path::Path) {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match export(segs, &[], titles, out, &settings(), &mut |_| {}, &cancel) {
        Ok(_) => {
            let d = cutlass_core::media::probe_duration_s(out).unwrap_or(-1.0);
            if d > 0.5 {
                println!("  PASS {name}");
            } else {
                println!("  FAIL {name}: produced {d:.2}s");
            }
        }
        Err(e) => {
            let msg = format!("{e:#}");
            println!("  FAIL {name}: {}", msg.lines().next().unwrap_or("").trim());
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let src = a.next().expect("usage: escaping_check <clip> <outdir>");
    let dir = std::path::PathBuf::from(a.next().expect("need outdir"));
    std::fs::create_dir_all(&dir)?;
    cutlass_core::media::ensure_ffmpeg()?;

    // Captions come from speech, so apostrophes are the common case, not an
    // edge case. The rest are characters ffmpeg's expression parser treats
    // specially and that a user can legitimately type into a title.
    println!("== title text ==");
    for (label, text) in [
        ("plain", "Hello there"),
        ("apostrophe", "It's working"),
        ("double quote", "She said \"hi\""),
        ("colon", "Chapter 1: Start"),
        ("percent", "100% done"),
        ("backslash", "path\\to\\thing"),
        ("comma", "one, two, three"),
        ("bracket+equals", "a[0]=1"),
        ("unicode", "café 中文 🎬"),
        ("newline", "line1\nline2"),
    ] {
        run(label, &one(&src), &[title(text)], &dir.join(format!("t_{}.mp4", label.replace([' ', '+'], "_"))));
    }

    // Media whose file names contain the same characters. The path reaches
    // ffmpeg as its own argument, but any filter that names a file (LUTs,
    // fonts) has to escape it inside the graph.
    println!("== media file names ==");
    for (label, name) in [
        ("apostrophe", "clip's take.mp4"),
        ("comma", "clip,two.mp4"),
        ("unicode", "café_clip.mp4"),
        ("bracket", "clip[1].mp4"),
        ("space", "my clip.mp4"),
    ] {
        let copy = dir.join(name);
        if std::fs::copy(&src, &copy).is_err() {
            println!("  SKIP {label}: filesystem rejected the name");
            continue;
        }
        run(
            label,
            &one(&copy.to_string_lossy()),
            &[],
            &dir.join(format!("m_{label}.mp4")),
        );
    }
    Ok(())
}
