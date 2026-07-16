//! Verifies the exact_frame path the desktop app uses: decode a frame,
//! strip alpha, JPEG-encode. `cargo run -p cutlass-engine --example
//! jpeg_check -- <video> <t>`

use cutlass_engine::MediaEngine;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: jpeg_check <video> <t>");
    let t: f64 = std::env::args().nth(2).unwrap_or_else(|| "1.0".into()).parse()?;

    let mut engine = MediaEngine::open(&path)?;
    let frame = engine.frame_at(t, 1920)?;
    let rgb: Vec<u8> = frame
        .data
        .chunks_exact(4)
        .flat_map(|px| [px[0], px[1], px[2]])
        .collect();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88).encode(
        &rgb,
        frame.width,
        frame.height,
        image::ExtendedColorType::Rgb8,
    )?;
    assert!(jpeg.starts_with(&[0xFF, 0xD8]), "not a JPEG");
    println!(
        "OK: {}x{} frame at pts {:.3}s -> {} byte JPEG",
        frame.width,
        frame.height,
        frame.pts_s,
        jpeg.len()
    );
    Ok(())
}
