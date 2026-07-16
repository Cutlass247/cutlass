# cutlass-engine — in-process decode results

Measured on the dev machine (debug build, 4K H.264 20s test clip),
`cargo run -p cutlass-engine --example engine_check -- <video> [proxy]`:

| Path | Result | Subprocess baseline (Spike B) |
|---|---|---|
| Sequential 4K decode (frame-threaded) | **261.6 fps** | 56.5 fps |
| Frame-accurate seek, all-intra 540p proxy | **6.2 ms** | 857 ms |
| Frame-accurate seek, long-GOP 4K source | 512 ms | 857 ms |
| Keyframe-fast seek, long-GOP 4K source | 97 ms | — |

## What this confirms

1. **In-process libav delivers the Spike B promise** — 138× faster seeks,
   4.6× decode throughput vs subprocess piping.
2. **libav decoders default to ONE thread.** `set_threading(Frame, 0)`
   took decode from 77 → 261 fps. Easy to miss, catastrophic to forget.
3. **Frame-accurate seek cost = GOP distance, not seek syscall.** The
   512 ms on source is decoding up to a GOP (~8s here) of frames to reach
   the target. All-intra proxies collapse it to 6 ms — the proxy-first
   architecture is confirmed as *the* scrub strategy, with keyframe-fast
   seek (97 ms) as the fallback while a proxy builds.
4. **Frame threading adds pipeline latency to single-frame seeks** —
   the decoder wants several packets before emitting frame one. A future
   dedicated seek path could use a low-delay decoder config.

## Licensing catch (product-relevant)

vendor/ffmpeg is the **LGPL** shared build — it decodes H.264/HEVC fine
but has **no libx264/x265 encoders** (those are GPL). Product proxy/export
encoding must use hardware encoders (QSV/NVENC/AMF — present in LGPL
builds), openh264, or ship a GPL-licensed build variant. Decision needed
before the export milestone.

## Build requirements (Windows)

- `FFMPEG_DIR` → repo `vendor/ffmpeg` (BtbN ffmpeg-n7.1 lgpl-shared)
- `LIBCLANG_PATH` → `C:\Program Files\LLVM\bin`
- Runtime: `vendor/ffmpeg/bin` on PATH or the 5 av*/sw* DLLs beside the exe
  (all three are set persistently on this machine; DLLs also copied to
  `target/debug/`)
