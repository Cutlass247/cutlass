# Spike B plan — Rust/wgpu media engine

**Question:** can we decode and scrub 4K H.264 at 60fps on a mid-range
Windows machine, in Rust, with GPU presentation via wgpu?

**Status: blocked on toolchain.** This machine has git + Node only. Needed:

1. **Rust toolchain** — `rustup-init.exe` from https://rustup.rs (~10 MB),
   which also requires the **MSVC Build Tools** (Visual Studio Build Tools
   with "Desktop development with C++", ~2–4 GB) for the `x86_64-pc-windows-msvc` target.
2. **FFmpeg shared libraries** — easiest path on Windows is
   `ffmpeg-sidecar` (downloads a prebuilt ffmpeg binary and pipes rawvideo
   over stdout) for the spike; the production engine graduates to
   `ffmpeg-next` bindings with hardware decode (NVDEC/QSV/D3D11VA).

## Spike design (1 crate, ~300 lines)

```
spikes/media-engine/
  Cargo.toml        # winit, wgpu, ffmpeg-sidecar, pollster
  src/main.rs
```

- Open a `winit` window with a `wgpu` surface (DX12 backend on Windows).
- Decode a 4K H.264 test file (generate one with ffmpeg's testsrc2 if no
  footage available) to NV12/RGBA frames.
- Upload frames as textures; render full-screen quad.
- **Scrub protocol:** map mouse-x to timeline position; on drag, seek and
  display nearest decoded frame.

## Measurements to capture (the actual deliverable)

| Metric | Target | Why |
|---|---|---|
| Sequential decode throughput | ≥ 60 fps at 4K | playback headroom |
| Seek-to-frame latency (cold) | ≤ 80 ms | scrub feel |
| Seek-to-frame latency (cached ±2s ring buffer) | ≤ 16 ms | butter scrub |
| Frame upload + present time | ≤ 4 ms | leaves budget for effects |
| Memory of a 120-frame 4K cache | (measure) | sizing the frame cache |

## Fallback ladder if targets miss

1. Scrub on half-res decode (skip loop filter / fast decode flags).
2. Scrub on pre-generated proxy (this is the production plan anyway —
   the spike then validates the proxy pipeline instead).
3. Hardware decode via D3D11VA before giving up on any target.

## Exit criteria

Write FINDINGS.md with the measured numbers on this machine's actual
hardware, and a go/no-go on "raw 4K scrubbing" vs "proxy-first from day
one."
