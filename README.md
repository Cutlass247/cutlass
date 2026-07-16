# Cutlass ⚔️

**Cut sharper. Own everything.**

Cutlass is a video editor built to take on Premiere Pro, DaVinci Resolve, Final Cut Pro, and CapCut by refusing each of their fatal flaws:

| Competitor's flaw | Cutlass answer |
|---|---|
| Premiere: crashes, subscription fatigue | Rust core, sandboxed plugins, one-time purchase option |
| Resolve: steep learning curve, hardware hog | Progressive-disclosure UI, proxy-first pipeline for laptops |
| Final Cut: Mac-only, no collaboration | Cross-platform (wgpu), CRDT-native multiplayer |
| CapCut: rights grabs, watermarks, paywalls | One-page ToS — your content is yours; free tier, no watermark |

## The five product wedges

1. **Local-first performance, cloud-native collaboration** — native GPU engine + Google-Docs-style multiplayer editing.
2. **Progressive disclosure UI** — CapCut-simple *Create* mode and full *Studio* mode over one shared project format. You graduate, you don't migrate.
3. **AI as copilot, not autopilot** — transcript editing, auto-cuts, captions, multicam sync, all as accept/reject suggestions on the timeline.
4. **Radical ownership** — open, documented project format; no rights grabs, ever.
5. **Pricing that respects users** — generous free tier, no watermarks, one-time purchase alongside optional cloud subscription.

## Architecture (short version)

- **Core engine:** Rust
- **GPU render graph:** wgpu (Vulkan / Metal / DX12)
- **Decode/encode:** FFmpeg + hardware acceleration (NVDEC / VideoToolbox / QSV)
- **Desktop shell:** Tauri 2, UI in React + TypeScript, timeline on canvas/WebGL
- **Project model:** Automerge CRDT — multiplayer, offline-first, undo/redo and history for free
- **Local AI:** whisper.cpp / ONNX Runtime
- **Plugins:** sandboxed WASM

Full detail in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## De-risking spikes

The two bets the company rests on, proven before anything else is built:

| Spike | Question it answers | Status |
|---|---|---|
| [`spikes/crdt-timeline`](spikes/crdt-timeline) | Can a video timeline be a CRDT? What are sane merge semantics for concurrent edits? | ✅ working |
| [`spikes/media-engine`](spikes/media-engine) | Can we decode + scrub 4K H.264 at 60fps in Rust/wgpu on a mid-range machine? | 📋 planned (needs Rust toolchain) |

## v1 scope (ruthless)

Great cutting · captions · transcript editing · color presets · multiplayer.
Curated codecs done perfectly: H.264, HEVC, ProRes, AV1.
**Not** in v1: node compositing (no Fusion clone), full DAW (no Fairlight clone), codec long-tail.
