//! Audit harness: what a half-written project file costs the user.
//! `cargo run -p cutlass-core --example project_robustness`

use cutlass_core::project::Project;

fn main() {
    let mut p = Project::new("Audit");
    for i in 0..40 {
        let id = format!("m{i}");
        p.set_media(&id, &id, &format!("C:/clips/take{i}.mp4"), 30.0).unwrap();
        p.add_clip(&cutlass_core::project::Clip {
            id: format!("c{i}"),
            name: id.clone(),
            media: id,
            track: "V1".into(),
            start: i as f64 * 3.0,
            len: 3.0,
            src_in: 0.0,
            text: String::new(),
            lut: String::new(),
            fx: Default::default(),
            kf: Default::default(),
        })
        .unwrap();
    }
    let bytes = p.save();
    println!("saved project: {} bytes, {} clips", bytes.len(), p.clips_state().len());

    // fs::write truncates before writing, so an interrupted save (crash, power
    // cut, or an external drive dropping out mid-write) leaves exactly this.
    for pct in [90, 50, 10] {
        let cut = bytes.len() * pct / 100;
        match Project::load(&bytes[..cut]) {
            Ok(mut q) => println!("  truncated to {pct:>3}%: loaded, {} clips", q.clips_state().len()),
            Err(e) => println!(
                "  truncated to {pct:>3}%: LOST -- {}",
                e.to_string().lines().next().unwrap_or("")
            ),
        }
    }
    // a single flipped byte, as a corrupted sector would give
    let mut bad = bytes.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xFF;
    match Project::load(&bad) {
        Ok(mut q) => println!("  one byte flipped: loaded, {} clips", q.clips_state().len()),
        Err(e) => println!(
            "  one byte flipped: LOST -- {}",
            e.to_string().lines().next().unwrap_or("")
        ),
    }
    // empty file: the state a crash right at the start of the write leaves
    match Project::load(&[]) {
        Ok(mut q) => println!("  empty file: loaded, {} clips", q.clips_state().len()),
        Err(e) => println!("  empty file: LOST -- {}", e.to_string().lines().next().unwrap_or("")),
    }
}
