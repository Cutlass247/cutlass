//! Print a project's timeline, to find what sits at a given moment.
//! `cargo run -p cutlass-core --example dump_project -- <file.cutlass> [seconds]`

use cutlass_core::project::Project;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("usage: dump_project <file.cutlass> [seconds]");
    let mark: Option<f64> = a.next().and_then(|s| s.parse().ok());

    let bytes = std::fs::read(&path)?;
    let mut p = Project::load(&bytes)?;
    let media: Vec<(String, String, String, f64)> = p.media_entries();
    let mut clips = p.clips_state();
    clips.sort_by(|x, y| {
        x.track
            .cmp(&y.track)
            .then(x.start.partial_cmp(&y.start).unwrap_or(std::cmp::Ordering::Equal))
    });

    println!("media:");
    for (id, name, path, dur) in &media {
        println!("  {id}  {dur:8.1}s  {name}  [{path}]");
    }
    println!("\nclips ({} total):", clips.len());
    for c in &clips {
        let end = c.start + c.len;
        let hit = match mark {
            Some(m) if m >= c.start && m < end => "  <<< the moment in question",
            _ => "",
        };
        let fx: Vec<String> = c
            .fx
            .iter()
            .filter(|(_, v)| v.abs() > 1e-9)
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        println!(
            "  {track:<4} {start:8.2}..{end:8.2} ({len:7.2}s) src_in={src_in:8.2} media={media} {text}{fx}{hit}",
            track = c.track,
            start = c.start,
            end = end,
            len = c.len,
            src_in = c.src_in,
            media = c.media,
            text = if c.text.is_empty() { String::new() } else { format!("text={:?} ", c.text) },
            fx = if fx.is_empty() { String::new() } else { format!("fx[{}]", fx.join(",")) },
        );
    }
    Ok(())
}
