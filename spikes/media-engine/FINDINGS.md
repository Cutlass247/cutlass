# Spike B findings — 4K decode/scrub benchmark

**Verdict: GO.** The hardware is not the problem; the architecture rule it
locks in: **decode must be in-process (libav bindings), never a subprocess
pipe.**

Machine: Intel Arc B580 (Vulkan backend), Windows 11. Test media: 20s
4K30 H.264 (testsrc2, libx264 veryfast). Run: `cargo run --release`.

## Measured

| Metric | Result | Target | |
|---|---|---|---|
| Decode, subprocess pipe (spike as written) | 56.5 fps | ≥ 60 | ❌ just under |
| Decode + GPU upload, subprocess pipe | 48.2 fps | ≥ 60 | ❌ |
| **Decode, no pipe (ffmpeg `-f null`, software)** | **236 fps** | ≥ 60 | ✅ 4× headroom |
| **Decode, hardware (d3d11va)** | **141 fps @ ~6× less CPU** (utime 2.6s vs 14.8s) | ≥ 60 | ✅ |
| Cold seek (fresh ffmpeg process per seek) | median 857 ms | ≤ 80 | ❌ (process spawn dominates) |
| Cached-frame GPU upload (12.4 MB yuv420p) | 12.7 ms | ≤ 16 | ✅ tight |
| 120-frame 4K cache | 1.49 GB RAM | — | sized |

## What this locks in

1. **In-process libav (`ffmpeg-next`), not subprocess piping.** Piping
   rawvideo over stdout cost ~75% of decode throughput (236 → 56.5 fps).
   Subprocess-per-seek is DOA (857 ms). This was the spike's design
   shortcut, now proven to be exactly the thing production must not do.
2. **Hardware decode by default** (D3D11VA/NVDEC/QSV/VideoToolbox). Same
   60fps+ result with ~6× less CPU — that CPU headroom is what runs
   Whisper transcription, waveform generation, and proxy encoding in the
   background while the user edits.
3. **Zero-copy is the next frontier:** with d3d11va the decoded frame is
   *already on the GPU*; sharing it into wgpu skips the 12.7 ms upload
   entirely. The upload path stays as fallback for software decode.
4. **Proxy-first stays mandatory** — not for decode speed, but because
   effects/color/compositing stack on top of decode, and mid-range
   laptops won't have a B580. Raw-4K scrubbing is a delighter on strong
   machines, not the baseline.

## Caveats

- Single 20s testsrc2 clip, one encoder setting, one machine. Real
  footage (high-bitrate camera files, long-GOP HEVC, 10-bit) will be
  slower; the 4× headroom is the cushion.
- `.rawvideo()` in ffmpeg-sidecar hard-codes rgb24 — override with
  explicit `-f rawvideo -pix_fmt yuv420p -` args (already fixed here).
