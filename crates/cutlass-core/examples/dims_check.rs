//! Sanity-check source dimension probing (drives the export upscale guard).
//! `cargo run -p cutlass-core --example dims_check -- <clip> [more...]`

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: dims_check <clip> [more...]");
        return;
    }
    for p in args {
        let (w, h) = cutlass_core::media::probe_dimensions(std::path::Path::new(&p));
        let name = std::path::Path::new(&p)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if w == 0 || h == 0 {
            println!("FAIL  {name}  (dimensions unknown)");
        } else {
            println!("OK    {name}  {w}x{h}");
        }
    }
}
