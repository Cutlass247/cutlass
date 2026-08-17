// IPC layer. In Tauri it calls the Rust commands; in a plain browser it
// serves a self-contained mock so the UI can be developed and verified
// without the native shell.

/// Effect Controls params (sparse; absent = identity default).
export type Fx = Record<string, number>;

export interface Clip {
  id: string;
  name: string;
  media: string;
  track: string;
  start: number;
  len: number;
  src_in: number;
  /// non-empty = a title (text) clip, no media.
  text?: string;
  /// path to a .cube 3D LUT applied to this clip (empty = none)
  lut?: string;
  fx?: Fx;
  /// param → (time-string → value); present params animate.
  kf?: Record<string, Record<string, number>>;
}

/// Track names are a kind letter + 1-based index: "V1".."Vn" (video,
/// composited bottom→top), "A1".."An" (audio-only beds). These helpers
/// are the single source of truth for parsing them across the app.
export type TrackKind = "video" | "audio";
export function trackKind(name: string): TrackKind {
  return name.charAt(0).toUpperCase() === "A" ? "audio" : "video";
}
export function trackIndex(name: string): number {
  return parseInt(name.slice(1), 10) || 1;
}

/// Sorted (time, value) points for a param's keyframes.
export function kfPoints(clip: Clip, param: string): [number, number][] {
  const m = clip.kf?.[param];
  if (!m) return [];
  return Object.entries(m)
    .map(([t, v]) => [parseFloat(t), v] as [number, number])
    .sort((a, b) => a[0] - b[0]);
}

/// Linear interpolation, clamped to the ends (mirrors core::interp).
export function interp(pts: [number, number][], t: number): number | null {
  if (pts.length === 0) return null;
  if (pts.length === 1) return pts[0][1];
  if (t <= pts[0][0]) return pts[0][1];
  if (t >= pts[pts.length - 1][0]) return pts[pts.length - 1][1];
  for (let i = 0; i < pts.length - 1; i++) {
    const [t0, v0] = pts[i];
    const [t1, v1] = pts[i + 1];
    if (t >= t0 && t <= t1) {
      const f = t1 - t0 < 1e-9 ? 0 : (t - t0) / (t1 - t0);
      return v0 + (v1 - v0) * f;
    }
  }
  return pts[pts.length - 1][1];
}

/// Effective value of a param at clip-relative time (keyframes win).
export function fxValueAt(clip: Clip, param: string, clipTime: number): number {
  const pts = kfPoints(clip, param);
  const v = interp(pts, clipTime);
  return v ?? fxValue(clip, param);
}

/// Identity-defaulted effect values for a clip.
export const FX_DEFAULTS: Record<string, number> = {
  brightness: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  tint: 0,
  hue: 0,
  blur: 0,
  sharpen: 0,
  grain: 0,
  vignette: 0,
  flip_h: 0,
  flip_v: 0,
  chroma: 0,
  chroma_sim: 0.3,
  denoise: 0,
  scale: 1,
  rot: 0,
  pos_x: 0,
  pos_y: 0,
  fade_in: 0,
  fade_out: 0,
  volume: 1,
  speed: 1,
  // censor boxes (up to 3): 0 off / 1 blur / 2 pixelate / 3 solid; x/y = box
  // centre, w/h = box size (fraction of frame), str = 0..1 strength.
  censor: 0,
  censor_x: 0.5,
  censor_y: 0.5,
  censor_w: 0.25,
  censor_h: 0.25,
  censor_str: 0.4,
  censor_color: 0,
  censor2: 0,
  censor2_x: 0.5,
  censor2_y: 0.5,
  censor2_w: 0.25,
  censor2_h: 0.25,
  censor2_str: 0.4,
  censor2_color: 0,
  censor3: 0,
  censor3_x: 0.5,
  censor3_y: 0.5,
  censor3_w: 0.25,
  censor3_h: 0.25,
  censor3_str: 0.4,
  censor3_color: 0,
};

export function fxValue(clip: Clip, key: string): number {
  return clip.fx?.[key] ?? FX_DEFAULTS[key] ?? 0;
}

export interface ProjectSnapshot {
  name: string;
  clips: Clip[];
}

export interface MediaItem {
  id: string;
  name: string;
  path: string;
  duration_s: number;
  /// native source size; 0 when unknown. Used to warn about upscaling.
  width?: number;
  height?: number;
  scrub_fps: number;
  thumbs: string[]; // data URLs, in time order
  waveform: number[]; // normalized peaks, whole file (empty = no audio)
}

export interface ImportResult {
  media: MediaItem;
  project: ProjectSnapshot;
}

export const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function pickVideo(): Promise<string | null> {
  if (!inTauri) return "mock://sample.mp4";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const file = await open({
    multiple: false,
    filters: [
      { name: "Video", extensions: ["mp4", "mov", "mkv", "webm", "avi", "m4v"] },
    ],
  });
  return typeof file === "string" ? file : null;
}

export async function importMedia(path: string): Promise<ImportResult> {
  if (!inTauri) return mockImport(path);
  return invoke<ImportResult>("import_media", { path });
}

/// Place a clip on the timeline from already-imported pool media (a
/// drag-and-drop from the media bin). Returns the new snapshot + clip id.
export async function addClipFromMedia(
  mediaId: string,
  track: string,
  start: number
): Promise<{ project: ProjectSnapshot; clipId: string }> {
  if (!inTauri) return mockAddClipFromMedia(mediaId, track, start);
  return invoke("add_clip_from_media", { mediaId, track, start });
}

/// Remove a track: deletes its clips and shifts higher same-kind tracks
/// down so numbering stays contiguous.
export async function removeTrack(track: string): Promise<ProjectSnapshot> {
  if (!inTauri) return mockRemoveTrack(track);
  return invoke("remove_track", { track });
}

export async function getProject(): Promise<ProjectSnapshot> {
  if (!inTauri) return structuredClone(mockState.project);
  return invoke<ProjectSnapshot>("get_project");
}

export async function moveClip(
  id: string,
  track: string,
  start: number
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      clip.track = track;
      clip.start = start;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("move_clip", { id, track, start });
}

export async function trimClip(
  id: string,
  start: number,
  len: number,
  srcIn: number
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      clip.start = start;
      clip.len = len;
      clip.src_in = srcIn;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("trim_clip", { id, start, len, srcIn });
}

/// Split a clip at timeline position `at` (the blade tool).
export async function splitClip(id: string, at: number): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      const rel = at - clip.start;
      if (rel > 0.02 && rel < clip.len - 0.02) {
        const speed = clip.fx?.speed ?? 1;
        mockState.project.clips.push({
          ...clip,
          id: `${id}-s${Date.now()}`,
          start: clip.start + rel,
          len: clip.len - rel,
          src_in: clip.src_in + rel * speed,
          kf: undefined,
        });
        clip.len = rel;
      }
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("split_clip", { id, at });
}

export async function removeClip(id: string, ripple: boolean): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clips = mockState.project.clips;
    const removed = clips.find((c) => c.id === id);
    mockState.project.clips = clips.filter((c) => c.id !== id);
    if (ripple && removed) {
      for (const c of mockState.project.clips) {
        if (c.track === removed.track && c.start > removed.start) {
          c.start = Math.max(0, c.start - removed.len);
        }
      }
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("remove_clip", { id, ripple });
}

/// Remove a media item from the project — also removes any clips that use
/// it. Returns the updated project.
export async function removeMedia(mediaId: string): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    delete mockState.media[mediaId];
    mockState.project.clips = mockState.project.clips.filter((c) => c.media !== mediaId);
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("remove_media", { mediaId });
}

/// Set one Effect Controls parameter on a clip (undoable).
export async function setEffect(
  id: string,
  key: string,
  value: number
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      clip.fx = { ...(clip.fx ?? {}), [key]: value };
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_effect", { id, key, value });
}

/// Apply several fx params in one undoable step (Effects-tab Look/effect).
export async function setEffects(
  id: string,
  params: Record<string, number>
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) clip.fx = { ...(clip.fx ?? {}), ...params };
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_effects", { id, params });
}

/// Set (or clear with "") the .cube LUT applied to a clip.
export async function setLut(id: string, path: string): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) clip.lut = path;
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_lut", { id, path });
}

/// Native picker for a .cube LUT file. Returns its path (mock returns a stub).
export async function pickLut(): Promise<string | null> {
  if (!inTauri) return "mock://look.cube";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const f = await open({ multiple: false, filters: [{ name: "Cube LUT", extensions: ["cube"] }] });
  return typeof f === "string" ? f : null;
}

/// Read a text file (the .cube contents) for the GPU preview.
export async function readTextFile(path: string): Promise<string> {
  if (!inTauri) return MOCK_CUBE;
  return invoke<string>("read_text_file", { path });
}

export interface CubeLut {
  size: number;
  data: Float32Array; // size^3 * 3, red varies fastest
}

/// Parse a .cube 3D LUT. Returns null if malformed.
export function parseCube(text: string): CubeLut | null {
  let size = 0;
  const data: number[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("LUT_3D_SIZE")) {
      size = parseInt(line.split(/\s+/)[1], 10);
      continue;
    }
    if (/^[A-Za-z_]/.test(line)) continue; // TITLE / DOMAIN_* / LUT_1D_SIZE
    const p = line.split(/\s+/).map(Number);
    if (p.length >= 3 && p.slice(0, 3).every((n) => !Number.isNaN(n))) data.push(p[0], p[1], p[2]);
  }
  if (!size || data.length !== size * size * size * 3) return null;
  return { size, data: new Float32Array(data) };
}

// a tiny warm 2^3 LUT so the browser demo shows the preview path working
const MOCK_CUBE = `LUT_3D_SIZE 2
0 0 0
1 0 0
0 1 0
1 1 0
0 0 0.75
1 0 0.75
0 1 0.75
1 1 0.75`;

export interface CaptionSpec {
  text: string;
  start: number;
  len: number;
}

/// Create a batch of styled caption clips (from the transcript) on V2.
export async function addCaptions(captions: CaptionSpec[]): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    for (const c of captions) {
      mockState.project.clips.push({
        id: `mock-cap-${mockState.nextClip++}`,
        name: "Caption",
        media: "",
        track: "V2",
        start: c.start,
        len: Math.max(0.3, c.len),
        src_in: 0,
        text: c.text,
        fx: { pos_y: 0.34, font_size: 46, title_bg: 0.55 },
      });
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("add_captions", { captions });
}

/// Add/update a keyframe for a param at clip-relative time (undoable).
export async function setKeyframe(
  id: string,
  param: string,
  t: number,
  value: number
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      const kf = { ...(clip.kf ?? {}) };
      kf[param] = { ...(kf[param] ?? {}), [t.toFixed(3)]: value };
      clip.kf = kf;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_keyframe", { id, param, t, value });
}

/// Remove all keyframes for a param.
export async function clearKeyframes(id: string, param: string): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip && clip.kf) {
      const kf = { ...clip.kf };
      delete kf[param];
      clip.kf = kf;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("clear_keyframes", { id, param });
}

/// Motion-track a censor box across its clip. Writes position keyframes for
/// `${prefix}_x` / `${prefix}_y` (replacing any prior track) and returns the
/// updated project. Runs offline template matching in the backend.
export async function trackCensor(
  id: string,
  prefix: string,
  cx: number,
  cy: number,
  bw: number,
  bh: number,
  anchor: number
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    await new Promise((r) => setTimeout(r, 700)); // simulate the tracking pass
    return mockTrackCensor(id, prefix, cx, cy);
  }
  return invoke<ProjectSnapshot>("track_censor", { id, prefix, cx, cy, bw, bh, anchor });
}

/// Fully clear a censor box (style, geometry, strength, colour, keyframes) so
/// re-adding the slot gives a clean default box.
export async function resetCensor(id: string, prefix: string): Promise<ProjectSnapshot> {
  if (!inTauri) return mockResetCensor(id, prefix);
  return invoke<ProjectSnapshot>("reset_censor", { id, prefix });
}

/// Subscribe to motion-tracking progress (0..1). No-op in the browser mock.
export async function onTrackProgress(cb: (p: number) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<number>("track-progress", (e) => cb(e.payload));
}

// browser-mock progress fan-out so the UI animation is testable without Tauri
const mockProgressCbs = new Set<(media: string, pct: number) => void>();

/// Subscribe to transcription progress (per media, 0..100).
export async function onTranscribeProgress(
  cb: (media: string, pct: number) => void
): Promise<() => void> {
  if (!inTauri) {
    mockProgressCbs.add(cb);
    return () => mockProgressCbs.delete(cb);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<{ media: string; pct: number }>("transcribe-progress", (e) =>
    cb(e.payload.media, e.payload.pct)
  );
}

/// Add a title clip on V2 at `start` (default lower-third style).
export async function addTitle(start: number): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    mockState.project.clips.push({
      id: `title-${Date.now()}`,
      name: "Title",
      media: "",
      track: "V2",
      start,
      len: 4,
      src_in: 0,
      text: "Title",
      fx: { pos_y: 0.3, font_size: 56, title_bg: 0.5 },
    });
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("add_title", { start });
}

export async function setTitleText(id: string, text: string): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) clip.text = text;
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_title_text", { id, text });
}

/// Set (dur=0 clears) a transition into a clip from its left neighbor.
export async function setTransition(
  id: string,
  dur: number,
  dip: boolean
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const clip = mockState.project.clips.find((c) => c.id === id);
    if (clip) {
      clip.fx = { ...(clip.fx ?? {}), trans_dur: dur, trans_dip: dip ? 1 : 0 };
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("set_transition", { id, dur, dip });
}

export async function currentRoom(): Promise<string | null> {
  if (!inTauri) return null;
  return invoke<string | null>("current_room");
}

export interface Presence {
  id: string;
  name: string;
  color: string;
  playhead: number;
}

export async function sendPresence(payload: Presence): Promise<void> {
  if (!inTauri) return;
  await invoke("send_presence", { payload });
}

export async function onPresence(cb: (p: Presence) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<Presence>("presence", (e) => cb(e.payload));
}

/// Render the V1 track to an MP4. Resolves to the encoder used, or null
/// if the user cancelled the file picker.
export interface ExportOptions {
  path: string;
  width: number;
  height: number;
  fps: number;
  format: string; // mp4_h264 | mp4_h265 | mov_prores | webm_vp9
  quality: string; // low | medium | high
  reframe?: string; // letterbox | fill | blur — how a non-matching source fills the frame
  reframe_x?: number; // 0..1 pan for fill (0.5 = centred)
  reframe_y?: number;
}

let mockExportCancelled = false;

/// Render the timeline with the chosen settings. Returns the encoder used.
export async function exportProject(opts: ExportOptions): Promise<string> {
  if (!inTauri) {
    mockExportCancelled = false;
    for (let i = 0; i < 24; i++) {
      await new Promise((r) => setTimeout(r, 70)); // simulate a render
      if (mockExportCancelled) throw new Error("export cancelled");
    }
    return "mock (h264)";
  }
  const { path, width, height, fps, format, quality, reframe, reframe_x, reframe_y } = opts;
  return invoke<string>("export_project", {
    path, width, height, fps, format, quality,
    reframe: reframe ?? "letterbox",
    reframe_x: reframe_x ?? 0.5,
    reframe_y: reframe_y ?? 0.5,
  });
}

/// Cancel the in-flight export (kills the ffmpeg render).
export async function cancelExport(): Promise<void> {
  if (!inTauri) {
    mockExportCancelled = true;
    return;
  }
  try {
    await invoke("cancel_export");
  } catch {
    /* best-effort */
  }
}

/// Pick a destination folder for the export (native directory dialog).
export async function pickExportDir(): Promise<string | null> {
  if (!inTauri) return "C:/Users/You/Videos";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const dir = await open({ directory: true, multiple: false });
  return typeof dir === "string" ? dir : null;
}

/// A sensible default export folder (Downloads, else home).
export async function defaultExportDir(): Promise<string> {
  if (!inTauri) return "C:/Users/You/Videos";
  try {
    const { downloadDir } = await import("@tauri-apps/api/path");
    return await downloadDir();
  } catch {
    return "";
  }
}

/// Open a URL or mailto: in the OS default handler.
export async function openUrl(url: string): Promise<void> {
  if (!inTauri) {
    window.open(url, "_blank");
    return;
  }
  try {
    await invoke("open_url", { url });
  } catch {
    /* best-effort */
  }
}

/// Reveal the exported file in the OS file browser.
export async function revealFile(path: string): Promise<void> {
  if (!inTauri) return;
  try {
    await invoke("reveal_file", { path });
  } catch {
    /* best-effort */
  }
}

export async function onExportProgress(
  cb: (progress: number) => void
): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<number>("export-progress", (e) => cb(e.payload));
}

/// Join a collab room on the sync relay.
export async function joinSession(room: string): Promise<void> {
  if (!inTauri) throw new Error("collab requires the desktop app");
  await invoke("join_session", { room });
}

/// Subscribe to project changes pushed by collab peers. Returns an
/// unsubscribe function (no-op in the browser mock).
export async function onProjectChanged(
  cb: (snap: ProjectSnapshot) => void
): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<ProjectSnapshot>("project-changed", (e) => cb(e.payload));
}

export async function undoEdit(): Promise<ProjectSnapshot> {
  if (!inTauri) {
    const prev = mockHist.undo.pop();
    if (prev) {
      mockHist.redo.push(structuredClone(mockState.project));
      mockState.project = prev;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("undo");
}

export async function redoEdit(): Promise<ProjectSnapshot> {
  if (!inTauri) {
    const next = mockHist.redo.pop();
    if (next) {
      mockHist.undo.push(structuredClone(mockState.project));
      mockState.project = next;
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("redo");
}

/// Razor several source ranges out in one undoable edit.
export async function cutRanges(
  mediaId: string,
  ranges: [number, number][]
): Promise<ProjectSnapshot> {
  if (!inTauri) {
    mockCheckpoint();
    const sorted = [...ranges].sort((a, b) => b[0] - a[0]);
    for (const [from, to] of sorted) {
      const mid = (from + to) / 2;
      const clip = mockState.project.clips.find(
        (c) => c.track === "V1" && c.media === mediaId && mid >= c.src_in && mid < c.src_in + c.len
      );
      if (clip) applyMockRazor(clip.id, from, to);
    }
    return structuredClone(mockState.project);
  }
  return invoke<ProjectSnapshot>("cut_ranges", { mediaId, ranges });
}

// mock "disk" for save/open in the browser
let mockSaved: { project: ProjectSnapshot } | null = null;

// Save the project. With `knownPath` (a prior save/open location) it writes
// there silently; otherwise it prompts (Save As). Returns the renamed
// snapshot and the path used, so the caller can remember it.
export async function saveProject(
  knownPath?: string
): Promise<{ project: ProjectSnapshot; path: string } | null> {
  if (!inTauri) {
    mockState.project.name = "My Project"; // simulate naming the saved file
    mockSaved = { project: structuredClone(mockState.project) };
    return { project: structuredClone(mockState.project), path: "My Project.cutlass" };
  }
  let path = knownPath;
  if (!path) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const picked = await save({
      filters: [{ name: "Cutlass project", extensions: ["cutlass"] }],
      defaultPath: "untitled.cutlass",
    });
    if (!picked) return null;
    path = picked;
  }
  // backend renames the project after the file and returns the new snapshot
  const project = await invoke<ProjectSnapshot>("save_project", { path });
  return { project, path };
}

export async function openProject(knownPath?: string): Promise<{
  project: ProjectSnapshot;
  media: MediaItem[];
  transcripts?: Record<string, Word[]>;
  path: string;
} | null> {
  if (!inTauri) {
    if (!mockSaved) return null;
    mockState.project = structuredClone(mockSaved.project);
    return { project: structuredClone(mockSaved.project), media: [], path: "My Project.cutlass" };
  }
  let path = knownPath;
  if (!path) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple: false,
      filters: [{ name: "Cutlass project", extensions: ["cutlass"] }],
    });
    if (typeof picked !== "string") return null;
    path = picked;
  }
  const res = await invoke<{
    project: ProjectSnapshot;
    media: MediaItem[];
    transcripts?: Record<string, Word[]>;
  }>("open_project", { path });
  return { ...res, path };
}

/// A .cutlass path the app was launched with (double-clicked file). Loaded
/// once on mount; null in the browser or on a plain launch.
export async function takeStartupFile(): Promise<string | null> {
  if (!inTauri) return null;
  return invoke("take_startup_file");
}

// App preferences persist to a disk settings file (WebView localStorage is
// not reliable across restarts); the browser mock falls back to localStorage.
export async function loadPrefs(): Promise<Record<string, unknown>> {
  if (!inTauri) {
    try {
      return JSON.parse(localStorage.getItem("cutlass-prefs") || "{}");
    } catch {
      return {};
    }
  }
  return invoke("load_prefs");
}

export async function savePref(key: string, value: unknown): Promise<void> {
  if (!inTauri) {
    let obj: Record<string, unknown> = {};
    try {
      obj = JSON.parse(localStorage.getItem("cutlass-prefs") || "{}");
    } catch {
      obj = {};
    }
    obj[key] = value;
    localStorage.setItem("cutlass-prefs", JSON.stringify(obj));
    return;
  }
  await invoke("save_pref", { key, value });
}

/// Fires when the running app is asked to open a .cutlass file (a second
/// double-click, routed here by single-instance). Payload is the path.
export async function onOpenFile(cb: (path: string) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const un = await listen<string>("open-file", (e) => cb(e.payload));
  return un;
}

/// Fires when the user tries to close the window (the OS close is
/// intercepted). The app decides whether to quit or prompt to save.
export async function onCloseRequested(cb: () => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const un = await listen("close-requested", () => cb());
  return un;
}

/// Actually close the window (after the save-on-quit choice is made).
export async function forceClose(): Promise<void> {
  if (!inTauri) return;
  await invoke("force_close");
}

/// Build thumbs for media that's in the project doc but not local yet.
export async function hydrateMedia(mediaId: string): Promise<MediaItem | null> {
  if (!inTauri) return null;
  try {
    return await invoke<MediaItem>("hydrate_media", { mediaId });
  } catch {
    return null; // source file missing on this machine
  }
}

export interface Word {
  text: string;
  start: number; // source-media seconds
  end: number;
}

/// On-device whisper transcription with word timestamps.
export async function transcribeMedia(mediaId: string): Promise<Word[]> {
  if (!inTauri) {
    for (let p = 0; p <= 100; p += 8) {
      mockProgressCbs.forEach((cb) => cb(mediaId, p));
      await new Promise((r) => setTimeout(r, 90));
    }
    return mockTranscript(mediaId);
  }
  return invoke<Word[]>("transcribe_media", { mediaId });
}

/// Delete a source range from a clip — the "delete these words" edit.
export async function razorOut(
  id: string,
  srcFrom: number,
  srcTo: number
): Promise<ProjectSnapshot> {
  if (!inTauri) return mockRazor(id, srcFrom, srcTo);
  return invoke<ProjectSnapshot>("razor_out", { id, srcFrom, srcTo });
}

/// Start timeline audio from `fromT`. False = no audio (browser mock or
/// no output device) — the UI falls back to its silent local clock.
export async function playAudio(fromT: number, muted: string[] = []): Promise<boolean> {
  if (!inTauri) return false;
  try {
    return await invoke<boolean>("play", { fromT, muted });
  } catch {
    return false;
  }
}

/// Stop audio; resolves to the timeline position where it stopped.
export async function pauseAudio(): Promise<number | null> {
  if (!inTauri) return null;
  try {
    return await invoke<number | null>("pause");
  } catch {
    return null;
  }
}

/// Real-time video frames streamed from the playback thread during play.
export async function onPlaybackFrame(
  cb: (t: number, src: string) => void
): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<{ t: number; src: string }>("playback-frame", (e) =>
    cb(e.payload.t, e.payload.src)
  );
}

export async function audioClock(): Promise<{ t: number; ended: boolean } | null> {
  if (!inTauri) return null;
  try {
    return await invoke<{ t: number; ended: boolean } | null>("playback_clock");
  } catch {
    return null;
  }
}

/// Full-quality frame from the native decode engine at source time `t`.
/// Returns null in browser-mock mode (the proxy frame stays up).
export async function exactFrame(path: string, t: number): Promise<string | null> {
  if (!inTauri) return null;
  return invoke<string>("exact_frame", { path, t });
}

// ── browser-only mock ──────────────────────────────────────────────────

const mockState: {
  project: ProjectSnapshot;
  nextClip: number;
  media: Record<string, MediaItem>;
} = {
  project: { name: "Untitled", clips: [] },
  nextClip: 1,
  media: {},
};

const mockHist: { undo: ProjectSnapshot[]; redo: ProjectSnapshot[] } = { undo: [], redo: [] };

function mockCheckpoint() {
  mockHist.undo.push(structuredClone(mockState.project));
  if (mockHist.undo.length > 100) mockHist.undo.shift();
  mockHist.redo = [];
}

function applyMockRazor(id: string, srcFrom: number, srcTo: number) {
  const clips = mockState.project.clips;
  const c = clips.find((x) => x.id === id);
  if (!c) return;
  const srcEnd = c.src_in + c.len;
  const from = Math.max(c.src_in, srcFrom);
  const to = Math.min(srcEnd, srcTo);
  const removed = to - from;
  if (removed <= 0) return;
  const leftLen = from - c.src_in;
  const rightLen = srcEnd - to;
  const origStart = c.start;
  const origEnd = c.start + c.len;
  if (rightLen > 1e-9) {
    clips.push({ ...c, id: `${id}-r${Date.now()}${Math.random().toString(36).slice(2, 6)}`, start: origStart + leftLen, len: rightLen, src_in: to });
  }
  if (leftLen > 1e-9) {
    c.len = leftLen;
  } else {
    clips.splice(clips.indexOf(c), 1);
  }
  for (const o of clips) {
    if (o.id !== id && o.track === c.track && o.start > origEnd - 1e-9) {
      o.start = Math.max(0, o.start - removed);
    }
  }
}

function mockThumbs(durationS: number, fps: number, label: string): string[] {
  const canvas = document.createElement("canvas");
  canvas.width = 480;
  canvas.height = 270;
  const ctx = canvas.getContext("2d")!;
  const n = Math.max(1, Math.round(durationS * fps));
  const thumbs: string[] = [];
  for (let i = 0; i < n; i++) {
    const t = i / fps;
    const hue = (t / durationS) * 300;
    const g = ctx.createLinearGradient(0, 0, 480, 270);
    g.addColorStop(0, `hsl(${hue}, 60%, 30%)`);
    g.addColorStop(1, `hsl(${hue + 40}, 60%, 12%)`);
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, 480, 270);
    ctx.fillStyle = "rgba(255,255,255,0.9)";
    ctx.font = "bold 36px system-ui";
    ctx.fillText(`${label}  ${t.toFixed(1)}s`, 24, 140);
    thumbs.push(canvas.toDataURL("image/jpeg", 0.7));
  }
  return thumbs;
}

function mockTranscript(mediaId: string): Promise<Word[]> {
  const media = mockState.project.clips.find((c) => c.media === mediaId);
  const dur = media ? media.len + media.src_in : 12;
  // includes fillers and a long pause so smart cuts have work to do
  const text =
    "so um the quick brown fox uh jumps over the lazy dog | cutlass makes um video editing fast and simple".split(" ");
  const spoken = text.filter((t) => t !== "|");
  const per = (dur - 1.6) / spoken.length;
  const words: Word[] = [];
  let t = 0;
  for (const w of text) {
    if (w === "|") {
      t += 1.6; // dead air
      continue;
    }
    words.push({ text: w, start: t, end: t + per * 0.9 });
    t += per;
  }
  return new Promise((r) => setTimeout(() => r(words), 600));
}

async function mockRazor(id: string, srcFrom: number, srcTo: number): Promise<ProjectSnapshot> {
  mockCheckpoint();
  applyMockRazor(id, srcFrom, srcTo);
  return structuredClone(mockState.project);
}

async function mockImport(path: string): Promise<ImportResult> {
  await new Promise((r) => setTimeout(r, 400)); // fake work
  mockCheckpoint();
  const n = mockState.nextClip++;
  const duration = 8 + n * 4;
  const fps = 5;
  const name = path.replace("mock://", "").replace(".mp4", `-${n}.mp4`);
  const media: MediaItem = {
    id: `mock-media-${n}`,
    name,
    path,
    duration_s: duration,
    width: 1920,
    height: 1080,
    scrub_fps: fps,
    thumbs: mockThumbs(duration, fps, name),
    waveform: Array.from({ length: 600 }, (_, i) => {
      const speech = Math.abs(Math.sin(i / 9)) * (0.55 + 0.45 * Math.abs(Math.sin(i / 41)));
      return Math.min(1, speech + Math.random() * 0.12);
    }),
  };
  // register in the pool only — no clip on the timeline until dragged in
  mockState.media[media.id] = media;
  return { media, project: structuredClone(mockState.project) };
}

function mockAddClipFromMedia(
  mediaId: string,
  track: string,
  start: number
): { project: ProjectSnapshot; clipId: string } {
  mockCheckpoint();
  const m = mockState.media[mediaId];
  const n = mockState.nextClip++;
  const clipId = `mock-clip-${n}`;
  mockState.project.clips.push({
    id: clipId,
    name: m?.name ?? mediaId,
    media: mediaId,
    track,
    start: Math.max(0, start),
    len: m?.duration_s ?? 8,
    src_in: 0,
  });
  return { project: structuredClone(mockState.project), clipId };
}

// Synthetic "tracker" for the browser mock: wander the box around its
// current centre so the follow behaviour is visible without a real file.
function mockResetCensor(id: string, prefix: string): ProjectSnapshot {
  mockCheckpoint();
  const clip = mockState.project.clips.find((c) => c.id === id);
  if (clip) {
    if (clip.kf) {
      const kf = { ...clip.kf };
      delete kf[`${prefix}_x`];
      delete kf[`${prefix}_y`];
      clip.kf = kf;
    }
    clip.fx = {
      ...(clip.fx ?? {}),
      [prefix]: 0,
      [`${prefix}_x`]: 0.5,
      [`${prefix}_y`]: 0.5,
      [`${prefix}_w`]: 0.25,
      [`${prefix}_h`]: 0.25,
      [`${prefix}_str`]: 0.4,
      [`${prefix}_color`]: 0,
    };
  }
  return structuredClone(mockState.project);
}

function mockTrackCensor(id: string, prefix: string, cx: number, cy: number): ProjectSnapshot {
  mockCheckpoint();
  const clip = mockState.project.clips.find((c) => c.id === id);
  if (clip) {
    const clampP = (v: number) => Math.min(0.95, Math.max(0.05, v));
    const kx: Record<string, number> = {};
    const ky: Record<string, number> = {};
    const n = Math.max(4, Math.round(clip.len * 8));
    for (let i = 0; i <= n; i++) {
      const t = (i / n) * clip.len;
      const p = i / n;
      kx[t.toFixed(3)] = clampP(cx + 0.18 * Math.sin(p * Math.PI * 3));
      ky[t.toFixed(3)] = clampP(cy + 0.1 * Math.sin(p * Math.PI * 5));
    }
    clip.kf = { ...(clip.kf ?? {}), [`${prefix}_x`]: kx, [`${prefix}_y`]: ky };
  }
  return structuredClone(mockState.project);
}

function mockRemoveTrack(track: string): ProjectSnapshot {
  mockCheckpoint();
  const audio = trackKind(track) === "audio";
  const removed = trackIndex(track);
  const kind = audio ? "A" : "V";
  mockState.project.clips = mockState.project.clips
    .filter((c) => c.track !== track)
    .map((c) =>
      trackKind(c.track) === (audio ? "audio" : "video") && trackIndex(c.track) > removed
        ? { ...c, track: `${kind}${trackIndex(c.track) - 1}` }
        : c
    );
  return structuredClone(mockState.project);
}

// ── licensing ────────────────────────────────────────────────────────
export interface LicenseInfo {
  status: "trial" | "paid" | "owner" | "expired" | "offline" | "error";
  active: boolean;
  days_left: number | null;
  machine_id: string;
  message: string;
  needs_online: boolean;
}

// Browser mock: flip states for UI dev via
//   localStorage.cutlassMockLicense = "trial" | "expired" | "paid" | "owner" | "offline"
function mockLicense(): LicenseInfo {
  const s = (localStorage.getItem("cutlassMockLicense") || "trial") as LicenseInfo["status"];
  const base = { machine_id: "mock0000deadbeef", needs_online: false };
  switch (s) {
    case "expired":
      return { status: "expired", active: false, days_left: 0,
        message: "Your 7-day free trial has ended. Purchase to keep editing.", ...base };
    case "paid":
      return { status: "paid", active: true, days_left: null, message: "Licensed — thank you.", ...base };
    case "owner":
      return { status: "owner", active: true, days_left: null, message: "Creator edition — full access.", ...base };
    case "offline":
      return { status: "offline", active: false, days_left: null,
        message: "Connect to the internet to start your 7-day free trial.", ...base, needs_online: true };
    default:
      return { status: "trial", active: true, days_left: 6,
        message: "6 days left in your free trial.", ...base };
  }
}

export async function licenseStatus(): Promise<LicenseInfo> {
  if (!inTauri) return mockLicense();
  return invoke<LicenseInfo>("license_status");
}

export async function licenseRedeem(code: string): Promise<LicenseInfo> {
  if (!inTauri) {
    const ok = /^CUTLASS-/i.test(code.trim());
    if (ok) localStorage.setItem("cutlassMockLicense", "paid");
    return ok
      ? { status: "paid", active: true, days_left: null, machine_id: "mock0000deadbeef",
          message: "Licensed — thank you.", needs_online: false }
      : { status: "error", active: false, days_left: null, machine_id: "mock0000deadbeef",
          message: "That code wasn't accepted. Check it and try again.", needs_online: false };
  }
  return invoke<LicenseInfo>("license_redeem", { code });
}

export async function licenseMachineId(): Promise<string> {
  if (!inTauri) return "mock0000deadbeef";
  return invoke<string>("license_machine_id");
}
