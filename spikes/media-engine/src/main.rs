//! Spike B — can we decode + scrub 4K H.264 at 60fps on this machine?
//!
//! Headless benchmark: decodes via an ffmpeg subprocess (ffmpeg-sidecar
//! pipes yuv420p rawvideo over stdout) and uploads frames to the GPU via
//! wgpu. Production would use in-process libav + hardware decode, so every
//! number here is an UPPER BOUND on cost (pipe transfer + process spawn
//! overhead included).
//!
//! Metrics vs targets (see PLAN.md):
//!   1. sequential decode throughput          — target ≥ 60 fps
//!   2. decode + GPU upload throughput        — target ≥ 60 fps
//!   3. cold seek to random position          — target ≤ 80 ms
//!   4. cached-frame present (upload only)    — target ≤ 16 ms

use std::path::Path;
use std::time::Instant;

use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::FfmpegEvent;

const W: u32 = 3840;
const H: u32 = 2160;
const FPS: u32 = 30;
const CLIP_SECONDS: u32 = 20;
const YUV420_FRAME_BYTES: usize = (W as usize * H as usize) * 3 / 2;

fn main() {
    println!("Spike B — 4K decode/scrub benchmark");
    println!("{}", "─".repeat(60));

    print!("ffmpeg: ");
    ffmpeg_sidecar::download::auto_download().expect("ffmpeg auto-download failed");
    println!("ready ({})", ffmpeg_sidecar::paths::ffmpeg_path().display());

    let test_file = "test4k.mp4";
    if !Path::new(test_file).exists() {
        generate_test_file(test_file);
    }

    let gpu = pollster::block_on(Gpu::new());
    println!("GPU: {}", gpu.name);
    println!();

    // ── 1 + 2: sequential decode, with and without GPU upload ──────────
    let decode_fps = bench_sequential(test_file, None);
    println!("1. decode only:        {decode_fps:.1} fps  (target ≥ 60)");
    let upload_fps = bench_sequential(test_file, Some(&gpu));
    println!("2. decode + upload:    {upload_fps:.1} fps  (target ≥ 60)");

    // ── 3: cold seeks to random positions ──────────────────────────────
    let mut latencies: Vec<f64> = (0..15)
        .map(|i| bench_cold_seek(test_file, (i as f64 * 1.31) % CLIP_SECONDS as f64))
        .collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = latencies[latencies.len() / 2];
    println!(
        "3. cold seek:          median {:.0} ms, min {:.0}, max {:.0}  (target ≤ 80; includes process spawn)",
        median,
        latencies[0],
        latencies[latencies.len() - 1]
    );

    // ── 4: cached-frame present = GPU upload of an in-RAM frame ────────
    let frame = vec![0x80u8; YUV420_FRAME_BYTES];
    gpu.upload(&frame); // warm-up
    let t0 = Instant::now();
    let reps = 100;
    for _ in 0..reps {
        gpu.upload(&frame);
    }
    let upload_ms = t0.elapsed().as_secs_f64() * 1000.0 / reps as f64;
    println!("4. cached present:     {upload_ms:.2} ms/frame  (target ≤ 16)");

    println!();
    println!(
        "frame cache sizing: 120 × 4K yuv420p frames = {:.2} GB RAM",
        120.0 * YUV420_FRAME_BYTES as f64 / 1e9
    );

    let pass = decode_fps >= 60.0 && upload_fps >= 60.0 && median <= 80.0 && upload_ms <= 16.0;
    println!("{}", "─".repeat(60));
    println!(
        "VERDICT: {}",
        if pass {
            "all targets met — raw 4K scrubbing viable on this machine"
        } else {
            "targets missed — proxy-first pipeline is mandatory (see PLAN.md fallback ladder)"
        }
    );
}

/// Generate a 20s 4K30 H.264 test clip with ffmpeg's testsrc2 pattern.
fn generate_test_file(path: &str) {
    println!("generating {path} (4K30 H.264, {CLIP_SECONDS}s) — one-time, takes a few minutes...");
    let status = FfmpegCommand::new()
        .args([
            "-f", "lavfi",
            "-i", &format!("testsrc2=size={W}x{H}:rate={FPS}"),
            "-t", &CLIP_SECONDS.to_string(),
            "-c:v", "libx264",
            "-preset", "veryfast",
            "-pix_fmt", "yuv420p",
            "-y", path,
        ])
        .spawn()
        .expect("spawn ffmpeg")
        .wait()
        .expect("ffmpeg encode failed");
    assert!(status.success(), "test file generation failed");
    println!("done.");
}

/// Decode the whole file; if `gpu` is given, also upload every frame.
/// Returns achieved frames/sec.
fn bench_sequential(path: &str, gpu: Option<&Gpu>) -> f64 {
    let mut child = FfmpegCommand::new()
        .input(path)
        // NOT .rawvideo(): that helper hard-codes rgb24; we want yuv420p
        // (12.4 MB/frame vs 24.9 — and it's what a real YUV pipeline ships).
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .spawn()
        .expect("spawn ffmpeg");

    let t0 = Instant::now();
    let mut frames = 0u32;
    for event in child.iter().expect("ffmpeg iter") {
        if let FfmpegEvent::OutputFrame(f) = event {
            assert_eq!(f.data.len(), YUV420_FRAME_BYTES, "unexpected frame size — pix_fmt override not applied?");
            if let Some(gpu) = gpu {
                gpu.upload(&f.data);
            }
            frames += 1;
        }
    }
    frames as f64 / t0.elapsed().as_secs_f64()
}

/// Time from "user drops playhead at ts" to first decoded frame, using a
/// fresh ffmpeg process with fast (pre-input) seek. Upper bound: includes
/// process spawn, which in-process libav avoids entirely.
fn bench_cold_seek(path: &str, ts: f64) -> f64 {
    let t0 = Instant::now();
    let mut child = FfmpegCommand::new()
        .args(["-ss", &format!("{ts:.3}")])
        .input(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .spawn()
        .expect("spawn ffmpeg");
    let mut latency = f64::NAN;
    for event in child.iter().expect("ffmpeg iter") {
        if let FfmpegEvent::OutputFrame(_) = event {
            latency = t0.elapsed().as_secs_f64() * 1000.0;
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(!latency.is_nan(), "no frame produced for seek to {ts}");
    latency
}

/// Minimal wgpu context. Frames are uploaded as a single R8Unorm texture of
/// W × H*3/2 — byte-identical to a production planar-YUV upload; the YUV→RGB
/// conversion would run in a shader at negligible cost next to the upload.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    name: String,
}

impl Gpu {
    async fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .expect("no GPU adapter found");
        let name = format!("{} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .expect("device request failed");
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d {
                width: W,
                height: H * 3 / 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self { device, queue, texture, name }
    }

    fn upload(&self, frame: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            frame,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W),
                rows_per_image: Some(H * 3 / 2),
            },
            wgpu::Extent3d {
                width: W,
                height: H * 3 / 2,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([]);
        self.device.poll(wgpu::Maintain::Wait);
    }
}
