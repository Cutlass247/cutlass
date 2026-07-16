// IPC layer. In Tauri it calls the Rust commands; in a plain browser it
// serves a self-contained mock so the UI can be developed and verified
// without the native shell.

export interface Clip {
  id: string;
  name: string;
  media: string;
  track: string;
  start: number;
  len: number;
  src_in: number;
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

export async function removeClip(id: string, ripple: boolean): Promise<ProjectSnapshot> {
  if (!inTauri) {
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

/// Full-quality frame from the native decode engine at source time `t`.
/// Returns null in browser-mock mode (the proxy frame stays up).
export async function exactFrame(path: string, t: number): Promise<string | null> {
  if (!inTauri) return null;
  return invoke<string>("exact_frame", { path, t });
}

// ── browser-only mock ──────────────────────────────────────────────────

const mockState: { project: ProjectSnapshot; nextClip: number } = {
  project: { name: "Untitled", clips: [] },
  nextClip: 1,
};

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

async function mockImport(path: string): Promise<ImportResult> {
  await new Promise((r) => setTimeout(r, 400)); // fake work
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
  };
  const end = mockState.project.clips
    .filter((c) => c.track === "V1")
    .reduce((m, c) => Math.max(m, c.start + c.len), 0);
  mockState.project.clips.push({
    id: `mock-clip-${n}`,
    name,
    media: media.id,
    track: "V1",
    start: end,
    len: duration,
    src_in: 0,
  });
  return { media, project: structuredClone(mockState.project) };
}
