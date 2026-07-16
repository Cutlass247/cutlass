# Cutlass Architecture

## System overview

```
┌─────────────────────────────────────────────────────┐
│  UI Layer (desktop app — React/TypeScript in Tauri) │
│  Create mode ←→ Studio mode (same project model)    │
├─────────────────────────────────────────────────────┤
│  Application Layer (Rust)                           │
│  Project model = CRDT document                      │
│  Command system (undo/redo = event log)             │
│  Plugin host (WASM sandbox)                         │
├──────────────────────────┬──────────────────────────┤
│  Media Engine (Rust)     │  AI Services             │
│  • Decode: FFmpeg +      │  • Local: Whisper        │
│    hw accel (NVDEC/      │    (captions/transcript),│
│    VideoToolbox/QSV)     │    scene detection,      │
│  • Render graph: wgpu    │    silence detection     │
│    (Vulkan/Metal/DX12)   │  • Cloud (optional):     │
│  • Frame cache + proxy   │    heavier models        │
│    pipeline (background) │                          │
├──────────────────────────┴──────────────────────────┤
│  Sync Service (cloud, optional)                     │
│  CRDT relay + presence · proxy streaming ·          │
│  render farm for exports                            │
└─────────────────────────────────────────────────────┘
```

## Load-bearing decisions

### 1. The project file is a CRDT
The entire project document (tracks, clips, effects, markers) lives in an
Automerge CRDT. Consequences:

- **Multiplayer is native**, not bolted on — concurrent edits merge
  automatically, offline edits merge on reconnect.
- **Undo/redo and version history fall out of the event log.**
- Only the small project document syncs in real time; media syncs lazily
  as proxies.
- Merge semantics for timeline-specific conflicts (two people move the
  same clip; delete vs. trim) are ours to define — see
  `spikes/crdt-timeline/FINDINGS.md`.

### 2. Edit on proxies, always
Background-generate lightweight proxies on import; the timeline operates
on proxies by default and swaps in full-res only for export/final
preview. This is how we run smoothly on laptops where Resolve chokes —
performance by architecture, not by requiring a $3,000 GPU.

### 3. GPU render graph, not fixed pipeline
Effects, transitions, and color ops compile into a node graph executed on
the GPU via `wgpu`, which targets Vulkan, Metal, and DirectX 12 from one
codebase.

### 4. Plugins are sandboxed WASM
Third-party effects can't crash the host or exfiltrate data. Makes an
effects marketplace safe from day one, and directly answers Premiere's
plugin-induced instability.

## Stack

| Layer | Choice | Why |
|---|---|---|
| Core engine | Rust | Native performance without C++'s crash surface |
| GPU | wgpu | One render codebase → Vulkan/Metal/DX12 |
| Decode/encode | FFmpeg (rust bindings) + platform hw accel | Every codec, battle-tested |
| Desktop shell | Tauri 2 | Rust-native, ~10× lighter than Electron |
| UI | React + TypeScript; timeline on canvas/WebGL | DOM for panels, canvas for the 60fps timeline |
| Collaboration | Automerge CRDT + WebSocket relay | Multiplayer + offline-first + free history |
| Local AI | whisper.cpp / ONNX Runtime | On-device captions/transcripts — private, free to run |
| Cloud (optional) | Postgres + object storage + GPU render workers | Sync/collab/cloud export only |

## Top risks and mitigations

1. **Media engine correctness** (frame-accurate seek, A/V sync, VFR, HDR,
   10-bit) — lean on FFmpeg; launch with a curated codec list done
   perfectly (H.264/HEVC/ProRes/AV1).
2. **Real-time preview performance** — proxy-first pipeline + aggressive
   frame caching; benchmark on mid-range laptops from day one.
3. **Timeline CRDT semantics** — semi-charted territory; de-risked first
   (spike A).
4. **Scope death** — v1 is cutting, captions, transcript editing, color
   presets, multiplayer. Nothing else.
5. **Distribution** — free tier + "you own your content" positioning
   aimed at the CapCut exodus.
