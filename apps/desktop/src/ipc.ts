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
  const { path, width, height, fps, format, quality } = opts;
  return invoke<string>("export_project", { path, width, height, fps, format, quality });
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

export async function saveProject(): Promise<boolean> {
  if (!inTauri) {
    mockSaved = { project: structuredClone(mockState.project) };
    return true;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const path = await save({
    filters: [{ name: "Cutlass project", extensions: ["cutlass"] }],
    defaultPath: "untitled.cutlass",
  });
  if (!path) return false;
  await invoke("save_project", { path });
  return true;
}

export async function openProject(): Promise<{
  project: ProjectSnapshot;
  media: MediaItem[];
} | null> {
  if (!inTauri) {
    if (!mockSaved) return null;
    mockState.project = structuredClone(mockSaved.project);
    return { project: structuredClone(mockSaved.project), media: [] };
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const path = await open({
    multiple: false,
    filters: [{ name: "Cutlass project", extensions: ["cutlass"] }],
  });
  if (typeof path !== "string") return null;
  return invoke("open_project", { path });
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
  if (!inTauri) return mockTranscript(mediaId);
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
