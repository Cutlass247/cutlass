import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Clip,
  MediaItem,
  Presence,
  ProjectSnapshot,
  CubeLut,
  Word,
  addCaptions,
  addClipFromMedia,
  addTitle,
  aiHighlights,
  aiUsage,
  AiUsage,
  audioClock,
  CaptionSpec,
  cloudTranscribe,
  parseCube,
  pickLut,
  readTextFile,
  setLut,
  cancelExport,
  clearKeyframes,
  currentRoom,
  cutRanges,
  defaultExportDir,
  exactFrame,
  ExportOptions,
  exportProject,
  FX_DEFAULTS,
  fxValueAt,
  kfPoints,
  pickExportDir,
  getProject,
  hydrateMedia,
  importMedia,
  inTauri,
  joinSession,
  moveClip,
  onExportProgress,
  onTrackProgress,
  onTranscribeProgress,
  onPlaybackFrame,
  onPresence,
  onProjectChanged,
  openProject,
  takeStartupFile,
  onOpenFile,
  onCloseRequested,
  forceClose,
  loadPrefs,
  savePref,
  openUrl,
  pauseAudio,
  pickVideo,
  playAudio,
  razorOut,
  redoEdit,
  removeClip,
  removeMedia,
  removeTrack,
  revealFile,
  saveProject,
  setWorkspaceMode,
  splitClip,
  sendPresence,
  setEffect,
  setEffects,
  setKeyframe,
  setTitleText,
  setTransition,
  trackCensor,
  resetCensor,
  trackIndex,
  trackKind,
  transcribeMedia,
  trimClip,
  undoEdit,
} from "./ipc";
import { Mode, TopBar } from "./components/TopBar";
import { MediaPanel } from "./components/MediaPanel";
import { Monitor, RESOLUTIONS, Resolution, CensorItem } from "./components/Monitor";
import { ClipFormat, CLIP_FORMATS, ClipFormatDef, ShortSeg } from "./components/ClipFormat";
import { Inspector } from "./components/Inspector";
import { ExportDialog } from "./components/ExportDialog";
import { PPS_MAX, PPS_MIN, TRACK_H, Timeline, TrackCtl } from "./components/Timeline";
import { fxStyle, Resizer } from "./components/ui";

const MAX_TRACKS = 12; // per kind — a sane ceiling on "as many as you want"
const TRIM_ZONE_PX = 10;
const MIN_LEN_S = 0.1;
const PPS_DEFAULT = 48;
const FILLERS = new Set(["um", "uh", "uhh", "umm", "erm", "er", "ah", "hmm", "mm", "mhm"]);
const cleanWord = (t: string) => t.toLowerCase().replace(/[^a-z']/g, "");
const SILENCE_GAP_S = 0.8;

/// Nearest scrub-proxy thumbnail for a source time.
function thumbAt(m: MediaItem, srcT: number): string {
  const idx = Math.min(m.thumbs.length - 1, Math.max(0, Math.floor(srcT * m.scrub_fps)));
  return m.thumbs[idx];
}

type DragState =
  | {
      kind: "move";
      clipId: string;
      track: string;
      start: number;
      grabOffsetS: number;
      /// the dragged clip's original start (delta = start - primaryStart)
      primaryStart: number;
      /// when moving a multi-selection: all selected clips' original placement
      /// (they shift by the same delta, keeping their own tracks). null = single.
      group: { id: string; start: number; track: string }[] | null;
    }
  | {
      kind: "trim-l" | "trim-r";
      clipId: string;
      start: number;
      len: number;
      srcIn: number;
      orig: { start: number; len: number; srcIn: number };
    };

export default function App() {
  // ── project + media state ───────────────────────────────────────────
  const [project, setProject] = useState<ProjectSnapshot>({ name: "Untitled", clips: [] });
  const [media, setMedia] = useState<Record<string, MediaItem>>({});
  const [dirty, setDirty] = useState(false);
  // the file this project is saved to (Save overwrites it; null → Save As)
  const [projectPath, setProjectPath] = useState<string | null>(null);
  // auto-save preference, persisted to disk (loaded on mount below)
  const [autoSave, setAutoSave] = useState(false);
  const [playhead, setPlayhead] = useState(0);
  const [drag, setDrag] = useState<DragState | null>(null);
  // multi-selection: the set of selected clip ids. `selected` is the single
  // "focused" clip the Inspector/effects act on — only when exactly one is
  // selected (multi-select is for timeline move/delete, not per-clip editing).
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const selected = selectedIds.length === 1 ? selectedIds[0] : null;
  const selectOne = useCallback((id: string | null) => setSelectedIds(id ? [id] : []), []);
  // marquee (rubber-band) selection box, in lanes content coords; null = idle
  const [marquee, setMarquee] = useState<{ x: number; y: number; w: number; h: number } | null>(null);
  // right-click menu on a clip (viewport coords + which clip)
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; clipId: string } | null>(null);
  // user-saved Looks (Effects tab), persisted in localStorage
  const [customLooks, setCustomLooks] = useState<{ name: string; params: Record<string, number> }[]>(
    () => {
      try {
        return JSON.parse(localStorage.getItem("cutlass-looks") || "[]");
      } catch {
        return [];
      }
    }
  );
  // parsed .cube LUTs, keyed by path, for the GPU preview
  const [lutCache, setLutCache] = useState<Record<string, CubeLut | null>>({});
  const lutLoading = useRef<Set<string>>(new Set());
  // a media-bin item being dragged toward the timeline (pointer-based, so
  // it works inside the Tauri webview where HTML5 drag events don't fire)
  const [mediaGhost, setMediaGhost] = useState<{ name: string; x: number; y: number } | null>(null);
  // which censor box (prefix) is currently being motion-tracked, if any
  const [tracking, setTracking] = useState<string | null>(null);
  const [trackProgress, setTrackProgress] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcripts, setTranscripts] = useState<Record<string, Word[]>>({});
  const [transcribing, setTranscribing] = useState<string | null>(null);
  // per-media transcription progress (0..100), shown on the Add captions button
  const [transcribeProgress, setTranscribeProgress] = useState<Record<string, number>>({});
  const [wordSel, setWordSel] = useState<{ media: string; a: number; b: number } | null>(null);
  const [room, setRoom] = useState<string | null>(null);
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const [feedbackText, setFeedbackText] = useState("");
  // shown when the user closes the window with unsaved changes
  const [quitPromptOpen, setQuitPromptOpen] = useState(false);
  // confirm removing a media item that's used by timeline clips
  const [removeMediaAsk, setRemoveMediaAsk] = useState<{
    id: string;
    name: string;
    clips: number;
  } | null>(null);
  // export flow: settings dialog → progress modal (running → done | error)
  const [exportOpen, setExportOpen] = useState(false);
  const [exportDir, setExportDir] = useState("");
  const [exportModal, setExportModal] = useState<
    | { phase: "running"; progress: number; name: string; cancelling: boolean }
    | { phase: "done"; path: string; encoder: string }
    | { phase: "error"; message: string }
    | null
  >(null);
  const [peers, setPeers] = useState<Record<string, Presence & { ts: number }>>({});

  // ── shell state ─────────────────────────────────────────────────────
  const [mode, setMode] = useState<Mode>(
    () => (localStorage.getItem("cutlass-mode") as Mode) || "create"
  );
  const [pps, setPps] = useState(PPS_DEFAULT);
  const [snap, setSnap] = useState(true);
  const [markers, setMarkers] = useState<number[]>([]);
  const [resolution, setResolution] = useState<Resolution>(RESOLUTIONS[1]);
  const [trackCtl, setTrackCtl] = useState<Record<string, TrackCtl>>({});
  const [leftW, setLeftW] = useState(268);
  const [rightW, setRightW] = useState(316);
  const [tlH, setTlH] = useState(280);

  // ── Create-tab clip maker ───────────────────────────────────────────
  // Output shape + how footage fills it; drives the monitor preview and export.
  const [clipFormat, setClipFormat] = useState<ClipFormatDef>(CLIP_FORMATS[0]);
  const [clipReframe, setClipReframe] = useState<"fill" | "blur">("fill");
  const [clipReframeX, setClipReframeX] = useState(0.5);
  // media id waiting on its transcript before the AI moment-finder can run
  const [wantAi, setWantAi] = useState<string | null>(null);
  // true while the AI is analyzing the transcript for standout moments
  const [aiFinding, setAiFinding] = useState(false);
  // how the Create tab transcribes: "fast" = cloud GPU (audio leaves machine),
  // "private" = on-device whisper (nothing leaves, slower). Persisted.
  const [transcribeMode, setTranscribeMode] = useState<"fast" | "private">(
    () => (localStorage.getItem("cutlass-stt-mode") as "fast" | "private") || "fast"
  );
  const chooseTranscribeMode = useCallback((m: "fast" | "private") => {
    setTranscribeMode(m);
    localStorage.setItem("cutlass-stt-mode", m);
    savePref("transcribeMode", m).catch(() => {});
  }, []);
  // auto-split / highlights: candidate shorts (source-time ranges) + loaded one
  const [shorts, setShorts] = useState<ShortSeg[]>([]);
  const [activeShort, setActiveShort] = useState<number | null>(null);
  // whether picking a moment also burns captions onto the clip. Off by default
  // — captions are opt-in, never applied automatically. Persisted.
  const [captionsOn, setCaptionsOn] = useState<boolean>(
    () => localStorage.getItem("cutlass-captions-on") === "1"
  );
  // monthly AI allowance readout (null = unknown/hidden). Refreshed after AI ops.
  const [usage, setUsage] = useState<AiUsage | null>(null);
  const refreshUsage = useCallback(() => {
    aiUsage().then(setUsage).catch(() => {});
  }, []);
  useEffect(() => {
    refreshUsage();
  }, [refreshUsage]);

  const lanesRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Switching tabs swaps to that workspace's independent project + media +
  // transcripts, so Create and Studio never affect each other. Transient UI
  // state (selection, playhead, drag, markers) resets to the incoming tab.
  const switchMode = useCallback(
    (m: Mode) => {
    if (m === mode) return;
    // remember the tab we're leaving so its save file + unsaved state come back
    saveStateRef.current[mode] = { path: projectPathRef.current, dirty: dirtyRef.current };
    setPlaying(false);
    pauseAudio().catch(() => {});
    setWorkspaceMode(m)
      .then((res) => {
        setMode(m);
        localStorage.setItem("cutlass-mode", m);
        setProject(res.project);
        const map: Record<string, MediaItem> = {};
        for (const md of res.media) map[md.id] = md;
        setMedia(map);
        setTranscripts(res.transcripts ?? {});
        setSelectedIds([]);
        setShorts([]);
        setActiveShort(null);
        setDrag(null);
        setWordSel(null);
        setMarkers([]);
        setPlayhead(0);
        // restore the tab we're entering — its own save file + unsaved state,
        // so Save/auto-save keep working per workspace and can't cross over
        const incoming = saveStateRef.current[m];
        setProjectPath(incoming.path);
        setDirty(incoming.dirty);
      })
      .catch((e) => setError(String(e)));
    },
    [mode]
  );
  // How many video / audio tracks exist. Grows on demand (add-track
  // buttons) and auto-bumps to cover any track a loaded/synced clip sits
  // on. Order shown top→bottom: Vn..V1 (video, higher composites on top),
  // then A1..An (audio beds) beneath.
  const [vCount, setVCount] = useState(2);
  const [aCount, setACount] = useState(1);
  const tracks = useMemo(() => {
    if (mode === "create") {
      // Create is a simplified view, but it must still show the user's
      // footage — otherwise a clip placed on V2 in Studio leaves Create
      // showing an empty timeline while the shared playhead animates over
      // it. Show the video track(s) that actually hold clips (usually just
      // one); fall back to V1 for an empty project so there's a drop lane.
      const vids = Array.from(
        new Set(project.clips.map((c) => c.track).filter((t) => trackKind(t) === "video"))
      ).sort((a, b) => trackIndex(b) - trackIndex(a));
      return vids.length ? vids : ["V1"];
    }
    const v = Array.from({ length: vCount }, (_, i) => `V${vCount - i}`);
    const a = Array.from({ length: aCount }, (_, i) => `A${i + 1}`);
    return [...v, ...a];
  }, [mode, vCount, aCount, project.clips]);
  const addVideoTrack = useCallback(() => setVCount((v) => Math.min(MAX_TRACKS, v + 1)), []);
  const addAudioTrack = useCallback(() => setACount((a) => Math.min(MAX_TRACKS, a + 1)), []);
  const setTrack = useCallback(
    (track: string, patch: Partial<TrackCtl>) =>
      setTrackCtl((prev) => {
        const base = prev[track] ?? { lock: false, mute: false, hide: false };
        return { ...prev, [track]: { ...base, ...patch } };
      }),
    []
  );
  const ctlOf = useCallback(
    (track: string): TrackCtl => trackCtl[track] ?? { lock: false, mute: false, hide: false },
    [trackCtl]
  );

  // every local edit funnels through this: snapshot in, dirty out
  const applyEdit = useCallback((snap: ProjectSnapshot) => {
    setProject(snap);
    setDirty(true);
  }, []);

  // remove a track (deletes its clips, shifts higher tracks down). V1/V2
  // are the protected defaults, so only V3+ and any A-track are removable.
  const onRemoveTrack = useCallback(
    (track: string) => {
      const audio = trackKind(track) === "audio";
      if (!audio && trackIndex(track) <= 2) return; // never the default video tracks
      removeTrack(track)
        .then((snap) => {
          applyEdit(snap);
          if (audio) setACount((a) => Math.max(0, a - 1));
          else setVCount((v) => Math.max(2, v - 1));
        })
        .catch((e) => setError(String(e)));
    },
    [applyEdit]
  );

  // ── presence identity ───────────────────────────────────────────────
  const me = useRef({ id: Math.random().toString(36).slice(2, 8), name: "", color: "" });
  if (!me.current.name) {
    me.current.name = `editor-${me.current.id.slice(0, 4)}`;
    me.current.color = `hsl(${((parseInt(me.current.id, 36) % 360) + 360) % 360}, 75%, 60%)`;
  }

  // latest real-time frame streamed from the playback thread
  const [playFrame, setPlayFrame] = useState<string | null>(null);

  // ── mount: load project, subscribe to events ────────────────────────
  useEffect(() => {
    // sync the backend to the tab we're restoring (Create/Studio are separate
    // workspaces) and load that one. Both are empty on a fresh launch.
    setWorkspaceMode(mode)
      .then((res) => {
        setProject(res.project);
        const map: Record<string, MediaItem> = {};
        for (const md of res.media) map[md.id] = md;
        setMedia(map);
        setTranscripts(res.transcripts ?? {});
      })
      .catch(() => getProject().then(setProject).catch((e) => setError(String(e))));
    currentRoom().then((r) => r && setRoom(r));
    defaultExportDir().then((d) => d && setExportDir(d));
    const un = onProjectChanged((snap) => setProject(snap));
    const unExport = onExportProgress((p) =>
      setExportModal((m) =>
        // never let the bar run backwards: encoder fallbacks/retries restart
        // ffmpeg's progress from 0, but to the user it's one export
        m && m.phase === "running" ? { ...m, progress: Math.max(m.progress, p) } : m
      )
    );
    const unTrack = onTrackProgress((p) => setTrackProgress(p));
    const unTx = onTranscribeProgress((media, pct) =>
      setTranscribeProgress((prev) => ({ ...prev, [media]: pct }))
    );
    const unFrame = onPlaybackFrame((_t, src) => setPlayFrame(src));
    const unPresence = onPresence((p) =>
      setPeers((prev) => ({ ...prev, [p.id]: { ...p, ts: Date.now() } }))
    );
    const prune = setInterval(() => {
      setPeers((prev) => {
        const now = Date.now();
        const next = Object.fromEntries(
          Object.entries(prev).filter(([, v]) => now - v.ts < 6000)
        );
        return Object.keys(next).length === Object.keys(prev).length ? prev : next;
      });
    }, 2000);
    return () => {
      un.then((f) => f());
      unExport.then((f) => f());
      unTrack.then((f) => f());
      unTx.then((f) => f());
      unFrame.then((f) => f());
      unPresence.then((f) => f());
      clearInterval(prune);
    };
  }, []);

  // ── derived clip state (drag overrides applied) ─────────────────────
  const clips = useMemo(() => {
    if (!drag) return project.clips;
    // group move: shift every selected clip by the same delta
    if (drag.kind === "move" && drag.group) {
      const delta = drag.start - drag.primaryStart;
      const ids = new Set(drag.group.map((g) => g.id));
      const origStart = new Map(drag.group.map((g) => [g.id, g.start]));
      return project.clips.map((c) =>
        ids.has(c.id) ? { ...c, start: Math.max(0, (origStart.get(c.id) ?? c.start) + delta) } : c
      );
    }
    return project.clips.map((c) => {
      if (c.id !== drag.clipId) return c;
      return drag.kind === "move"
        ? { ...c, track: drag.track, start: drag.start }
        : { ...c, start: drag.start, len: drag.len, src_in: drag.srcIn };
    });
  }, [project, drag]);

  // a loaded or synced project may hold clips on tracks we don't show yet
  // — grow the lane counts so nothing is invisible
  useEffect(() => {
    let maxV = 0;
    let maxA = 0;
    for (const c of project.clips) {
      const i = trackIndex(c.track);
      if (trackKind(c.track) === "audio") maxA = Math.max(maxA, i);
      else maxV = Math.max(maxV, i);
    }
    if (maxV > 0) setVCount((v) => Math.max(v, maxV));
    if (maxA > 0) setACount((a) => Math.max(a, maxA));
  }, [project.clips]);

  const contentEndS = useMemo(
    () => clips.reduce((m, c) => Math.max(m, c.start + c.len), 0),
    [clips]
  );
  const timelineEndS = Math.max(30, contentEndS + 5);
  const selectedClip = useMemo(
    () => clips.find((c) => c.id === selected) ?? null,
    [clips, selected]
  );
  // keep clip rectangles (lanes content coords) current for marquee hit-testing
  useEffect(() => {
    clipRectsRef.current = clips.map((c) => {
      const lane = tracks.indexOf(c.track);
      return {
        id: c.id,
        left: c.start * pps,
        top: (lane < 0 ? 0 : lane) * TRACK_H,
        width: c.len * pps,
        height: TRACK_H,
      };
    });
  }, [clips, tracks, pps]);

  // ── playback (audio owns the clock when live) ───────────────────────
  const playheadRef = useRef(0);
  useEffect(() => {
    playheadRef.current = playhead;
  }, [playhead]);
  const playingRef = useRef(false);
  useEffect(() => {
    playingRef.current = playing;
  }, [playing]);
  // mirror of `dirty` for the window-close handler (avoids a stale closure)
  const dirtyRef = useRef(dirty);
  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);
  const projectPathRef = useRef(projectPath);
  useEffect(() => {
    projectPathRef.current = projectPath;
  }, [projectPath]);
  // each tab (Create/Studio) remembers its OWN save file + unsaved state, so
  // switching workspaces doesn't lose where a project was saved.
  const saveStateRef = useRef<Record<Mode, { path: string | null; dirty: boolean }>>({
    create: { path: null, dirty: false },
    studio: { path: null, dirty: false },
  });
  // latest tracks / zoom / media for the pointer-drag drop closure
  const tracksRef = useRef(tracks);
  useEffect(() => {
    tracksRef.current = tracks;
  }, [tracks]);
  const ppsRef = useRef(pps);
  useEffect(() => {
    ppsRef.current = pps;
  }, [pps]);
  const modeRef = useRef(mode);
  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);
  const mediaRef = useRef(media);
  useEffect(() => {
    mediaRef.current = media;
  }, [media]);
  const contentEndRef = useRef(0);
  useEffect(() => {
    contentEndRef.current = contentEndS;
  }, [contentEndS]);
  const trackCtlRef = useRef(trackCtl);
  useEffect(() => {
    trackCtlRef.current = trackCtl;
  }, [trackCtl]);
  // current clip ids, for keyboard Select-All without re-binding the listener
  const clipIdsRef = useRef<string[]>([]);
  useEffect(() => {
    clipIdsRef.current = project.clips.map((c) => c.id);
  }, [project.clips]);
  const selectedIdsRef = useRef<string[]>([]);
  useEffect(() => {
    selectedIdsRef.current = selectedIds;
  }, [selectedIds]);
  // clip rectangles in lanes content coords, for marquee hit-testing
  const clipRectsRef = useRef<{ id: string; left: number; top: number; width: number; height: number }[]>([]);

  // Playback is built from a timeline snapshot taken when play() starts, so a
  // mid-play edit (e.g. removing a clip) would otherwise keep playing the stale
  // snapshot. This signature changes whenever the audio/video-relevant timeline
  // does, which re-runs the effect below and restarts the engine in sync.
  const playSig = useMemo(() => {
    const clips = project.clips
      .map(
        (c) =>
          `${c.id}|${c.track}|${c.start}|${c.len}|${c.src_in}|${c.media}|${c.text ? "t" : ""}|${
            c.fx?.volume ?? 1
          }|${c.fx?.speed ?? 1}`
      )
      .join(",");
    const mutes = Object.entries(trackCtl)
      .filter(([, v]) => v.mute)
      .map(([t]) => t)
      .sort()
      .join(",");
    return `${clips}##${mutes}`;
  }, [project.clips, trackCtl]);

  useEffect(() => {
    if (!playing) return;
    let cancelled = false;
    let audioLive = false;
    const muted = Object.entries(trackCtlRef.current)
      .filter(([, c]) => c.mute)
      .map(([t]) => t);
    playAudio(playheadRef.current, muted).then((ok) => {
      if (!cancelled) audioLive = ok;
    });
    let last = performance.now();
    const local = setInterval(() => {
      const now = performance.now();
      const dt = (now - last) / 1000;
      last = now;
      setPlayhead((t) => {
        const nt = t + dt;
        if (!audioLive && nt >= contentEndRef.current) {
          setPlaying(false);
          return contentEndRef.current;
        }
        return nt;
      });
    }, 33);
    const sync = setInterval(async () => {
      if (!audioLive) return;
      const c = await audioClock();
      if (cancelled || !c) return;
      setPlayhead(c.t);
      if (c.ended) setPlaying(false);
    }, 250);
    return () => {
      cancelled = true;
      clearInterval(local);
      clearInterval(sync);
      setPlayFrame(null); // drop the streamed frame; back to proxy/engine
      pauseAudio().then((t) => {
        if (t != null) setPlayhead(t);
      });
    };
  }, [playing, playSig]);

  // broadcast presence while in a room
  useEffect(() => {
    if (!room) return;
    const timer = setInterval(() => {
      sendPresence({ ...me.current, playhead: playheadRef.current }).catch(() => {});
    }, 300);
    return () => clearInterval(timer);
  }, [room]);

  // ── program monitor: stacked video layers, bottom→top = V1..Vn ───────
  // each visible video track contributes at most one clip under the
  // playhead; they composite in order so higher tracks overlay lower.
  const videoLayers = useMemo(() => {
    const out: { clip: Clip; media: MediaItem; srcT: number }[] = [];
    const vtracks = tracks
      .filter((t) => trackKind(t) === "video")
      .sort((a, b) => trackIndex(a) - trackIndex(b)); // ascending = bottom→top
    for (const track of vtracks) {
      if (ctlOf(track).hide) continue;
      const clip = clips.find(
        (c) =>
          c.track === track && !c.text && playhead >= c.start && playhead < c.start + c.len
      );
      if (!clip) continue;
      const m = media[clip.media];
      if (!m || m.thumbs.length === 0) continue;
      const speed = clip.fx?.speed ?? 1;
      const srcT = clip.src_in + (playhead - clip.start) * speed;
      out.push({ clip, media: m, srcT: Math.min(srcT, m.duration_s) });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clips, media, playhead, tracks, trackCtl]);

  // topmost layer drives transcript/caption context + the settled hq frame
  const underPlayhead = videoLayers.length ? videoLayers[videoLayers.length - 1] : null;

  const [hq, setHq] = useState<{ key: string; src: string } | null>(null);
  const hqKey = underPlayhead
    ? `${underPlayhead.media.id}@${underPlayhead.srcT.toFixed(3)}`
    : null;
  useEffect(() => {
    if (!hqKey || !underPlayhead || drag || playing) return;
    const { media: m, srcT } = underPlayhead;
    const timer = setTimeout(async () => {
      try {
        const src = await exactFrame(m.path, srcT);
        if (src) setHq({ key: hqKey, src });
      } catch {
        /* proxy frame stays up */
      }
    }, 250);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hqKey, drag, playing]);

  // ── effects: live preview draft for the selected clip ───────────────
  const [fxDraft, setFxDraft] = useState<Record<string, number>>({});
  useEffect(() => setFxDraft({}), [selected]); // reset when selection changes
  // live title-text draft: typing previews on the monitor before the blur
  // commit, so titles read WYSIWYG just like the effect sliders
  const [textDraft, setTextDraft] = useState<string | null>(null);
  useEffect(() => setTextDraft(null), [selected]);

  // Selecting a clip parks the playhead on it (only when the playhead is
  // outside the clip) so the monitor shows the very clip being inspected —
  // this is what makes Inspector edits preview live, instead of against
  // some other frame the selected clip isn't even under.
  useEffect(() => {
    if (!selected || playingRef.current) return; // never yank the playhead mid-play
    const clip = clips.find((c) => c.id === selected);
    if (!clip) return;
    const ph = playheadRef.current;
    if (ph < clip.start || ph >= clip.start + clip.len) setPlayhead(clip.start);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  // fxStyle for a clip, merging the selected clip's live drag draft
  const layerStyle = useCallback(
    (clip: Clip): React.CSSProperties => {
      const draftApplies = clip.id === selected && Object.keys(fxDraft).length > 0;
      const effClip = draftApplies
        ? { ...clip, fx: { ...(clip.fx ?? {}), ...fxDraft } }
        : clip;
      return fxStyle(effClip, playhead);
    },
    [selected, fxDraft, playhead]
  );

  // what the monitor renders, bottom→top. While playing, the real-time
  // streamed frame stands in for the topmost layer; other layers hold
  // their proxy frame beneath it.
  const monitorLayers = useMemo(() => {
    return videoLayers.map((l, i) => {
      const top = i === videoLayers.length - 1;
      const live = playing && playFrame && top ? playFrame : null;
      const settled = top && hq && hq.key === hqKey ? hq.src : null;
      const draft = l.clip.id === selected ? fxDraft : {};
      return {
        key: l.clip.id,
        src: live ?? settled ?? thumbAt(l.media, l.srcT),
        style: layerStyle(l.clip),
        chroma: (draft.chroma ?? l.clip.fx?.chroma ?? 0) > 0.5,
        chromaSim: draft.chroma_sim ?? l.clip.fx?.chroma_sim ?? 0.3,
        lut: l.clip.lut ? lutCache[l.clip.lut] ?? null : null,
      };
    });
  }, [videoLayers, playing, playFrame, hq, hqKey, layerStyle, selected, fxDraft, lutCache]);

  // vignette is a monitor overlay (not a CSS filter) — driven by the
  // topmost clip, honouring the selected clip's live drag draft
  const monitorOverlay = useMemo(() => {
    const clip = underPlayhead?.clip;
    if (!clip) return { vignette: 0, grain: 0 };
    const draft = clip.id === selected ? fxDraft : {};
    const val = (k: string) => draft[k] ?? clip.fx?.[k] ?? 0;
    return { vignette: val("vignette"), grain: val("grain") };
  }, [underPlayhead, selected, fxDraft]);

  const onFxPreview = useCallback(
    (key: string, v: number) => setFxDraft((d) => ({ ...d, [key]: v })),
    []
  );
  const onFxCommit = useCallback(
    (key: string, v: number) => {
      if (!selected) return;
      setEffect(selected, key, v)
        .then((snap) => {
          applyEdit(snap);
          setFxDraft({}); // drop the drag draft so real/keyframed values show
        })
        .catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // Effects tab: drop a Look / effect preset (a bundle of fx) on the clip
  const onApplyEffect = useCallback(
    (params: Record<string, number>) => {
      if (!selected) return;
      setEffects(selected, params)
        .then((snap) => {
          applyEdit(snap);
          setFxDraft({});
        })
        .catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // Is this effect currently engaged on the selected clip? True when every
  // distinguishing param sits on the preset's side of its default — so a
  // tuned value (e.g. Blur nudged from 6 to 10) still reads as "on", and
  // opposite presets (Warmer vs Cooler) stay mutually exclusive.
  const effectActive = useCallback(
    (params: Record<string, number>) => {
      const fx = selectedClip?.fx ?? {};
      return Object.entries(params).every(([k, v]) => {
        const def = FX_DEFAULTS[k] ?? 0;
        if (v === def) return true; // this param doesn't distinguish the effect
        const cur = fx[k] ?? def;
        return v > def ? cur > def + 1e-6 : cur < def - 1e-6;
      });
    },
    [selectedClip]
  );

  // Click an effect chip: apply it, or — if it's already on — toggle it off
  // by resetting its params to their defaults. No more undo-to-remove.
  const onToggleEffect = useCallback(
    (params: Record<string, number>) => {
      if (!selected) return;
      const next = effectActive(params)
        ? Object.fromEntries(Object.keys(params).map((k) => [k, FX_DEFAULTS[k] ?? 0]))
        : params;
      onApplyEffect(next);
    },
    [selected, effectActive, onApplyEffect]
  );

  // load (parse) any .cube LUT a clip references, once, for the GPU preview
  useEffect(() => {
    for (const c of project.clips) {
      const path = c.lut;
      if (!path || lutLoading.current.has(path)) continue;
      lutLoading.current.add(path);
      readTextFile(path)
        .then((txt) => setLutCache((m) => ({ ...m, [path]: parseCube(txt) })))
        .catch(() => setLutCache((m) => ({ ...m, [path]: null })));
    }
  }, [project.clips]);

  const onImportLut = useCallback(async () => {
    if (!selected) return;
    const path = await pickLut();
    if (!path) return;
    try {
      applyEdit(await setLut(selected, path));
      if (!lutLoading.current.has(path)) {
        lutLoading.current.add(path);
        readTextFile(path)
          .then((txt) => setLutCache((m) => ({ ...m, [path]: parseCube(txt) })))
          .catch(() => {});
      }
    } catch (e) {
      setError(String(e));
    }
  }, [selected, applyEdit]);
  const onRemoveLut = useCallback(async () => {
    if (!selected) return;
    try {
      applyEdit(await setLut(selected, ""));
    } catch (e) {
      setError(String(e));
    }
  }, [selected, applyEdit]);

  // Custom Looks: save the selected clip's colour grade as a reusable Look
  const LOOK_KEYS = ["brightness", "contrast", "saturation", "temperature", "tint", "hue", "vignette"];
  const persistLooks = useCallback((looks: typeof customLooks) => {
    setCustomLooks(looks);
    localStorage.setItem("cutlass-looks", JSON.stringify(looks));
  }, []);
  const onSaveLook = useCallback(() => {
    if (!selectedClip) return;
    const params: Record<string, number> = {};
    for (const k of LOOK_KEYS) params[k] = selectedClip.fx?.[k] ?? FX_DEFAULTS[k] ?? 0;
    const name = window.prompt("Name this Look:", `My Look ${customLooks.length + 1}`);
    if (!name) return;
    persistLooks([...customLooks.filter((l) => l.name !== name), { name: name.trim(), params }]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedClip, customLooks, persistLooks]);
  const onDeleteLook = useCallback(
    (name: string) => persistLooks(customLooks.filter((l) => l.name !== name)),
    [customLooks, persistLooks]
  );

  // Ken Burns: a slow push-in via scale keyframes across the clip
  const onKenBurns = useCallback(() => {
    if (!selectedClip) return;
    const id = selectedClip.id;
    const end = Math.max(0.1, selectedClip.len);
    setKeyframe(id, "scale", 0, 1.0)
      .then(() => setKeyframe(id, "scale", end, 1.18))
      .then((snap) => applyEdit(snap))
      .catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedClip, applyEdit]);

  // a clip can take a transition if another clip on its track ends where
  // it begins (adjacency)
  const selectedHasLeftNeighbor = useMemo(() => {
    if (!selectedClip) return false;
    return project.clips.some(
      (c) =>
        c.id !== selectedClip.id &&
        c.track === selectedClip.track &&
        Math.abs(c.start + c.len - selectedClip.start) < 1e-3
    );
  }, [selectedClip, project]);

  const onSetTransition = useCallback(
    (dur: number, dip: boolean) => {
      if (!selected) return;
      setTransition(selected, dur, dip).then(applyEdit).catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // clip-relative playhead time, for keyframe placement
  const clipTime = useMemo(() => {
    if (!selectedClip) return 0;
    return Math.min(selectedClip.len, Math.max(0, playhead - selectedClip.start));
  }, [selectedClip, playhead]);

  const onSetKeyframe = useCallback(
    (key: string, t: number, v: number) => {
      if (!selected) return;
      setKeyframe(selected, key, t, v)
        .then((snap) => {
          applyEdit(snap);
          setFxDraft({}); // keyframed value comes from the doc now
        })
        .catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );
  const onClearKeyframes = useCallback(
    (key: string) => {
      if (!selected) return;
      clearKeyframes(selected, key).then(applyEdit).catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // Censor box shown in the Monitor for the selected clip when it's under
  // the playhead. Position/size are keyframe-aware (fxValueAt), so the box
  // sits where the animation puts it; the live drag draft wins while dragging.
  const monitorCensors = useMemo((): CensorItem[] => {
    if (!selectedClip || selectedClip.text) return [];
    const onScreen =
      playhead >= selectedClip.start && playhead < selectedClip.start + selectedClip.len;
    if (!onScreen) return [];
    const prefixes = ["censor", "censor2", "censor3"];
    const out: CensorItem[] = [];
    prefixes.forEach((pre, slot) => {
      const style = Math.round(fxDraft[pre] ?? selectedClip.fx?.[pre] ?? 0);
      if (style <= 0) return;
      const at = (suf: string) => {
        const key = pre + suf;
        return fxDraft[key] ?? fxValueAt(selectedClip, key, clipTime);
      };
      out.push({ slot, style, x: at("_x"), y: at("_y"), w: at("_w"), h: at("_h"), str: at("_str"), color: at("_color") });
    });
    return out;
  }, [selectedClip, fxDraft, playhead, clipTime]);

  // Drag/resize a box from the Monitor. Preview merges into the fx draft; on
  // commit each param writes a keyframe at the playhead if it's already
  // keyframed (so dragging refines the track) otherwise the base value.
  const onCensor = useCallback(
    (slot: number, partial: Partial<{ x: number; y: number; w: number; h: number }>, commit: boolean) => {
      if (!selected || !selectedClip) return;
      const pre = ["censor", "censor2", "censor3"][slot] ?? "censor";
      const keyMap: Record<string, string> = {
        x: `${pre}_x`,
        y: `${pre}_y`,
        w: `${pre}_w`,
        h: `${pre}_h`,
      };
      const fxPartial: Record<string, number> = {};
      for (const [k, v] of Object.entries(partial)) fxPartial[keyMap[k]] = v as number;
      if (!commit) {
        setFxDraft((d) => ({ ...d, ...fxPartial }));
        return;
      }
      const base: Record<string, number> = {};
      const kfWrites: [string, number][] = [];
      for (const [pk, v] of Object.entries(fxPartial)) {
        if (kfPoints(selectedClip, pk).length > 0) kfWrites.push([pk, v]);
        else base[pk] = v;
      }
      const run = async () => {
        let snap: ProjectSnapshot | null = null;
        if (Object.keys(base).length) snap = await setEffects(selected, base);
        for (const [pk, v] of kfWrites) snap = await setKeyframe(selected, pk, clipTime, v);
        if (snap) applyEdit(snap);
        setFxDraft({});
      };
      run().catch((e) => setError(String(e)));
    },
    [selected, selectedClip, clipTime, applyEdit]
  );

  // Remove a censor box: fully clear the slot (params + tracked keyframes) so
  // re-adding it gives a fresh default box, not the one just removed.
  const onRemoveCensor = useCallback(
    (prefix: string) => {
      if (!selected) return;
      resetCensor(selected, prefix)
        .then((snap) => {
          applyEdit(snap);
          setFxDraft({});
        })
        .catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // Motion-track a censor box: the backend follows the patch and writes
  // position keyframes; we just apply the returned project.
  const onTrackCensor = useCallback(
    (prefix: string) => {
      if (!selected || !selectedClip) return;
      // the box exactly as the user placed it, at the current playhead frame
      const box = (suf: string) =>
        fxDraft[`${prefix}${suf}`] ?? fxValueAt(selectedClip, `${prefix}${suf}`, clipTime);
      setTracking(prefix);
      setTrackProgress(0);
      trackCensor(selected, prefix, box("_x"), box("_y"), box("_w"), box("_h"), clipTime)
        .then((snap) => {
          applyEdit(snap);
          setFxDraft({});
        })
        .catch((e) => setError(String(e)))
        .finally(() => setTracking(null));
    },
    [selected, selectedClip, clipTime, fxDraft, applyEdit]
  );

  const onAddTitle = useCallback(() => {
    addTitle(playheadRef.current)
      .then((snap) => {
        applyEdit(snap);
        // select the newest title so the editor opens
        const t = snap.clips.filter((c) => c.text).slice(-1)[0];
        if (t) selectOne(t.id);
      })
      .catch((e) => setError(String(e)));
  }, [applyEdit]);

  const onSetTitleText = useCallback(
    (text: string) => {
      if (!selected) return;
      setTitleText(selected, text)
        .then((snap) => {
          applyEdit(snap);
          setTextDraft(null); // committed text now lives in the doc
        })
        .catch((e) => setError(String(e)));
    },
    [selected, applyEdit]
  );

  // titles visible under the playhead → monitor overlay
  const titleOverlay = useMemo(() => {
    const active = clips.filter(
      (c) => c.text && playhead >= c.start && playhead < c.start + c.len
    );
    if (active.length === 0) return null;
    return active.map((c) => {
      // the selected title previews its live drafts (style + typed text)
      const sel = c.id === selected;
      const fx = sel ? { ...(c.fx ?? {}), ...fxDraft } : c.fx ?? {};
      const text = sel && textDraft !== null ? textDraft : c.text;
      const fs = (fx.font_size ?? 56) / 1080; // fraction of frame height
      const bg = fx.title_bg ?? 0;
      const px = (fx.pos_x ?? 0) * 100;
      const py = (fx.pos_y ?? 0) * 100;
      return (
        <div
          key={c.id}
          className="title-overlay"
          style={{ transform: `translate(${px}%, ${py}%)` }}
        >
          <span
            style={{
              fontSize: `${fs * 100}cqh`,
              background: bg > 0 ? `rgba(0,0,0,${bg})` : "transparent",
              padding: bg > 0 ? "0.1em 0.4em" : 0,
            }}
          >
            {text}
          </span>
        </div>
      );
    });
  }, [clips, playhead, selected, fxDraft, textDraft]);

  // ── import / transcribe ─────────────────────────────────────────────
  // `cloud` = Groq GPU transcription (fast, audio leaves the machine); else the
  // on-device whisper pass (private, slower). Both stream the same progress.
  const doTranscribe = useCallback(
    async (mediaId: string, cloud = false): Promise<Word[] | null> => {
      setTranscribing(mediaId);
      setTranscribeProgress((p) => ({ ...p, [mediaId]: 0 }));
      try {
        const words = cloud ? await cloudTranscribe(mediaId) : await transcribeMedia(mediaId);
        setTranscripts((t) => ({ ...t, [mediaId]: words }));
        setTranscribeProgress((p) => ({ ...p, [mediaId]: 100 }));
        setDirty(true); // transcript is now stored in the doc → savable
        return words;
      } catch (e) {
        setError(String(e));
        return null;
      } finally {
        setTranscribing(null);
        if (cloud) refreshUsage(); // cloud transcription counts against the allowance
      }
    },
    [refreshUsage]
  );

  const doImport = useCallback(async () => {
    setError(null);
    const path = await pickVideo();
    if (!path) return;
    setBusy(`Importing ${path.split(/[\\/]/).pop()}…`);
    try {
      const res = await importMedia(path);
      setMedia((m) => ({ ...m, [res.media.id]: res.media }));
      applyEdit(res.project);
      // No auto-transcribe on import anymore — "Find the best moments" fetches
      // the transcript on demand (cloud is fast enough that a head-start pass
      // would just waste CPU, or upload audio the user didn't ask to send yet).
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, doTranscribe]);

  // place a clip from a bin item at a given track + start
  const onDropMedia = useCallback(
    async (mediaId: string, track: string, start: number) => {
      try {
        const placed = await addClipFromMedia(mediaId, track, start);
        applyEdit(placed.project);
        selectOne(placed.clipId);
      } catch (e) {
        setError(String(e));
      }
    },
    [applyEdit]
  );

  // Pointer-based drag of a media-bin item onto the timeline. Pointer
  // events (unlike HTML5 drag) fire inside the Tauri webview, so this
  // works in the native app. A ghost follows the cursor; releasing over
  // the lanes drops a clip on the track/time under the pointer.
  const onMediaPointerDown = useCallback(
    (mediaId: string, e: React.PointerEvent) => {
      if ((e.target as HTMLElement).closest("button")) return; // let buttons click
      e.preventDefault();
      const name = mediaRef.current[mediaId]?.name ?? "";
      let moved = false;
      setMediaGhost({ name, x: e.clientX, y: e.clientY });
      const move = (ev: PointerEvent) => {
        moved = true;
        setMediaGhost({ name, x: ev.clientX, y: ev.clientY });
      };
      const up = (ev: PointerEvent) => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
        setMediaGhost(null);
        const lanes = lanesRef.current;
        if (!moved || !lanes) return;
        const rect = lanes.getBoundingClientRect();
        if (
          ev.clientX < rect.left ||
          ev.clientX > rect.right ||
          ev.clientY < rect.top ||
          ev.clientY > rect.bottom
        )
          return; // released outside the timeline
        const lane = Math.min(
          tracksRef.current.length - 1,
          Math.max(0, Math.floor((ev.clientY - rect.top) / TRACK_H))
        );
        // Create anchors the clip to the very start (one short, ready to
        // trim/export); Studio is a free multi-clip timeline, so it drops at
        // the cursor position.
        const start =
          modeRef.current === "create" ? 0 : Math.max(0, (ev.clientX - rect.left) / ppsRef.current);
        onDropMedia(mediaId, tracksRef.current[lane], start);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    },
    [onDropMedia]
  );

  // hydrate media known to the doc but not local (open/collab)
  const hydrating = useRef<Set<string>>(new Set());
  useEffect(() => {
    for (const c of project.clips) {
      if (media[c.media] || hydrating.current.has(c.media)) continue;
      hydrating.current.add(c.media);
      hydrateMedia(c.media).then((m) => {
        hydrating.current.delete(c.media);
        if (m) setMedia((prev) => ({ ...prev, [m.id]: m }));
      });
    }
  }, [project, media]);

  // ── save / open / export / collab ───────────────────────────────────
  // Save to the current file when we have one; only prompt (Save As) for a
  // never-saved project or an explicit Save As. `forcePrompt` forces the
  // dialog even when a path exists.
  const saveTo = useCallback(
    async (forcePrompt: boolean) => {
      const res = await saveProject(forcePrompt ? undefined : projectPath ?? undefined);
      if (res) {
        setProject(res.project); // picks up the new name → title stops saying Untitled
        setProjectPath(res.path);
        setDirty(false);
      }
      return !!res;
    },
    [projectPath]
  );
  const doSave = useCallback(async () => {
    try {
      await saveTo(false);
    } catch (e) {
      setError(String(e));
    }
  }, [saveTo]);
  const doSaveAs = useCallback(async () => {
    try {
      await saveTo(true);
    } catch (e) {
      setError(String(e));
    }
  }, [saveTo]);

  // ── save-on-close guard ─────────────────────────────────────────────
  // The window close is intercepted natively; quit straight away when there
  // are no unsaved changes, otherwise prompt Save / Don't save / Cancel.
  useEffect(() => {
    const un = onCloseRequested(() => {
      if (dirtyRef.current) setQuitPromptOpen(true);
      else forceClose();
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  const onQuitSave = useCallback(async () => {
    try {
      const saved = await saveTo(false); // may prompt for a location first time
      if (saved) forceClose();
      else setQuitPromptOpen(false); // save was cancelled → stay in the app
    } catch (e) {
      setError(String(e));
      setQuitPromptOpen(false);
    }
  }, [saveTo]);
  const onQuitDiscard = useCallback(() => forceClose(), []);

  const applyOpened = useCallback(
    (res: {
      project: ProjectSnapshot;
      media: MediaItem[];
      transcripts?: Record<string, Word[]>;
      path: string;
    }) => {
      setProject(res.project);
      const map: Record<string, MediaItem> = {};
      for (const m of res.media) map[m.id] = m;
      setMedia(map);
      setSelectedIds([]);
      setWordSel(null);
      setTranscripts(res.transcripts ?? {});
      setPlayhead(0);
      setDirty(false);
      setMarkers([]);
      setProjectPath(res.path);
      // this project has a file, so honour the saved auto-save preference
      loadPrefs()
        .then((p) => setAutoSave(p.autoSave === true))
        .catch(() => {});
    },
    []
  );

  const doOpen = useCallback(
    async (knownPath?: string) => {
      try {
        const res = await openProject(knownPath);
        if (res) applyOpened(res);
      } catch (e) {
        setError(String(e));
      }
    },
    [applyOpened]
  );

  // Auto-save starts OFF for a new untitled project — it has no file to
  // write to, so showing it on would be a lie. The saved preference is
  // applied when a project is opened (see applyOpened), which is the only
  // time auto-save can actually do anything.
  const toggleAutoSave = useCallback(() => {
    setAutoSave((v) => {
      const next = !v;
      savePref("autoSave", next).catch(() => {});
      return next;
    });
  }, []);

  // Auto-save: when enabled and the project has a file + unsaved changes,
  // write it back after a short idle (debounced so we don't save mid-edit).
  useEffect(() => {
    if (!autoSave || !dirty || !projectPath) return;
    const t = setTimeout(() => {
      saveTo(false).catch(() => {});
    }, 1500);
    return () => clearTimeout(t);
  }, [autoSave, dirty, projectPath, project, saveTo]);

  // Load a project the app was launched with (double-clicked .cutlass), and
  // handle a second double-click routed here by single-instance.
  useEffect(() => {
    takeStartupFile()
      .then((p) => {
        if (p) doOpen(p);
      })
      .catch(() => {});
    const un = onOpenFile((p) => doOpen(p));
    return () => {
      un.then((f) => f());
    };
  }, [doOpen]);

  // Tallest source actually on the timeline — the export dialog uses it to
  // default the resolution and warn about upscaling.
  const sourceHeight = useMemo(() => {
    let h = 0;
    for (const c of clips) {
      const m = media[c.media];
      if (m?.height) h = Math.max(h, m.height);
    }
    return h;
  }, [clips, media]);

  // Export button / File menu → open the settings dialog
  const doExport = useCallback(() => {
    setError(null);
    setExportOpen(true);
  }, []);

  // Beta feedback → open the user's mail client with a prefilled report
  const onSendFeedback = useCallback(() => {
    const body = `${feedbackText}\n\n---\nCutlass 0.1.0 (beta) · ${navigator.userAgent}`;
    const url = `mailto:cutlass.beta@gmail.com?subject=${encodeURIComponent(
      "Cutlass beta feedback"
    )}&body=${encodeURIComponent(body)}`;
    openUrl(url);
    setFeedbackOpen(false);
    setFeedbackText("");
  }, [feedbackText]);

  // dialog confirmed → run the render, driving the progress modal
  const runExport = useCallback(async (opts: ExportOptions) => {
    setExportOpen(false);
    const name = opts.path.split(/[\\/]/).pop() ?? "export";
    setExportModal({ phase: "running", progress: 0, name, cancelling: false });
    try {
      const encoder = await exportProject(opts);
      setExportModal({ phase: "done", path: opts.path, encoder });
    } catch (e) {
      const msg = String(e);
      // a user cancel just closes the modal — it isn't an error
      if (/cancel/i.test(msg)) setExportModal(null);
      else setExportModal({ phase: "error", message: msg });
    }
  }, []);

  const onCancelExport = useCallback(() => {
    cancelExport();
    setExportModal((m) => (m && m.phase === "running" ? { ...m, cancelling: true } : m));
  }, []);

  // Create-tab one-click export: render straight to the chosen shape (with
  // the Fill/Blur reframe) — no settings dialog, just pick a folder and go.
  const exportClip = useCallback(async () => {
    setError(null);
    const dir = (await pickExportDir()) ?? exportDir;
    if (!dir) return;
    const sep = dir.includes("\\") ? "\\" : "/";
    const stamp = new Date().toISOString().slice(0, 10);
    const path = `${dir}${dir.endsWith(sep) ? "" : sep}cutlass-${clipFormat.id}-${stamp}.mp4`;
    await runExport({
      path,
      width: clipFormat.w,
      height: clipFormat.h,
      fps: 30,
      format: "mp4_h264",
      quality: "high",
      reframe: clipReframe,
      reframe_x: clipReframeX,
      reframe_y: 0.5,
    });
  }, [clipFormat, clipReframe, clipReframeX, exportDir, runExport]);

  const doCollab = useCallback(async () => {
    const name = window.prompt("Room name (share it with your collaborator):", "cutlass-demo");
    if (!name) return;
    try {
      await joinSession(name.trim());
      setRoom(name.trim());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const doUndo = useCallback(
    () => undoEdit().then(applyEdit).catch((e) => setError(String(e))),
    [applyEdit]
  );
  const doRedo = useCallback(
    () => redoEdit().then(applyEdit).catch((e) => setError(String(e))),
    [applyEdit]
  );
  const doDeleteSel = useCallback(
    (ripple: boolean) => {
      const ids = selectedIds;
      if (ids.length === 0) return;
      setSelectedIds([]);
      (async () => {
        try {
          let snap: ProjectSnapshot | null = null;
          // ripple only makes sense one-at-a-time; delete right-to-left so
          // earlier ripples don't move clips we're about to remove
          const order = ripple ? [...ids].reverse() : ids;
          for (const id of order) snap = await removeClip(id, ripple);
          if (snap) applyEdit(snap);
        } catch (e) {
          setError(String(e));
        }
      })();
    },
    [selectedIds, applyEdit]
  );

  // remove media from the project (drops it + any clips that use it)
  const doRemoveMedia = useCallback(
    async (mediaId: string) => {
      try {
        let snap = await removeMedia(mediaId);
        // removeMedia drops the media + the clips that use it, but auto-captions
        // (text clips on V2, media="") were generated FROM that footage and are
        // now orphaned — sweep any that no longer sit over a real media clip.
        const mediaClips = snap.clips.filter((c) => c.media && !c.text);
        const orphanCaptions = snap.clips.filter(
          (c) =>
            c.text &&
            c.name === "Caption" &&
            !mediaClips.some((m) => c.start < m.start + m.len && c.start + c.len > m.start)
        );
        for (const c of orphanCaptions) snap = await removeClip(c.id, false);
        setProject(snap);
        setMedia((m) => {
          const n = { ...m };
          delete n[mediaId];
          return n;
        });
        // drop its transcript + any moments found for it
        setTranscripts((t) => {
          const n = { ...t };
          delete n[mediaId];
          return n;
        });
        setShorts([]);
        setActiveShort(null);
        // drop any now-deleted clips from the selection
        setSelectedIds((prev) => prev.filter((id) => snap.clips.some((c) => c.id === id)));
        setDirty(true);
      } catch (e) {
        setError(String(e));
      }
    },
    [selected, project]
  );
  // clicking a bin item's ✕ — confirm first if it's on the timeline
  const onRemoveMedia = useCallback(
    (mediaId: string) => {
      const clips = project.clips.filter((c) => c.media === mediaId).length;
      if (clips > 0) {
        setRemoveMediaAsk({ id: mediaId, name: media[mediaId]?.name ?? "this media", clips });
      } else {
        doRemoveMedia(mediaId);
      }
    },
    [project, media, doRemoveMedia]
  );

  // blade: split the clip at the playhead (selected clip if the playhead
  // is inside it, else the topmost clip under the playhead)
  const doSplit = useCallback(() => {
    const ph = playheadRef.current;
    const within = (c: Clip) => ph > c.start + 0.02 && ph < c.start + c.len - 0.02;
    const sel = selected ? project.clips.find((c) => c.id === selected) : null;
    let target = sel && within(sel) ? sel : null;
    if (!target) {
      for (const track of tracks) {
        const c = project.clips.find((c) => c.track === track && within(c));
        if (c) {
          target = c;
          break;
        }
      }
    }
    if (!target) return;
    splitClip(target.id, ph).then(applyEdit).catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, project, applyEdit]);

  // ── keyboard ────────────────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // don't hijack keys while the user is typing in any text field —
      // textareas (feedback, titles) and contenteditable, not just inputs
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable))
        return;
      if (e.ctrlKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        e.shiftKey ? doRedo() : doUndo();
      } else if (e.ctrlKey && e.key.toLowerCase() === "y") {
        e.preventDefault();
        doRedo();
      } else if (e.ctrlKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        e.shiftKey ? doSaveAs() : doSave();
      } else if (e.ctrlKey && e.key.toLowerCase() === "o") {
        e.preventDefault();
        doOpen();
      } else if (e.ctrlKey && e.key.toLowerCase() === "a") {
        e.preventDefault();
        setSelectedIds(clipIdsRef.current);
      } else if (e.code === "Space") {
        e.preventDefault();
        setPlaying((p) => !p);
      } else if (e.key === "Home") {
        setPlayhead(0);
      } else if (e.key === "End") {
        setPlayhead(contentEndRef.current);
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        const step = (e.shiftKey ? 1 : 1 / 30) * (e.key === "ArrowLeft" ? -1 : 1);
        setPlayhead((t) => Math.max(0, t + step));
      } else if (e.key.toLowerCase() === "s" && !e.ctrlKey) {
        e.preventDefault();
        doSplit();
      } else if (e.key.toLowerCase() === "m") {
        setMarkers((ms) => {
          const t = playheadRef.current;
          const near = ms.findIndex((m) => Math.abs(m - t) < 0.2);
          return near >= 0 ? ms.filter((_, i) => i !== near) : [...ms, t].sort((a, b) => a - b);
        });
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        doDeleteSel(e.shiftKey);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, doSave, doSaveAs, doOpen, doUndo, doRedo, doDeleteSel, doSplit]);

  // ── scrubbing + clip dragging (with snapping) ───────────────────────
  const capture = (e: React.PointerEvent) => {
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* drag still works while hovered */
    }
  };

  const scrubTo = useCallback(
    (clientX: number) => {
      const lanes = lanesRef.current;
      if (!lanes) return;
      setPlayhead(Math.max(0, (clientX - lanes.getBoundingClientRect().left) / pps));
    },
    [pps]
  );

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      setPps((z) => Math.min(PPS_MAX, Math.max(PPS_MIN, z * (e.deltaY < 0 ? 1.25 : 0.8))));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // keep the playhead on screen — the timeline follows it (page-flips
  // forward during playback, recenters on a seek) instead of letting it
  // march off the right edge. Skipped while dragging a clip.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || drag) return;
    const x = playhead * pps;
    const left = el.scrollLeft;
    const view = el.clientWidth;
    const margin = 48;
    if (x < left + margin) {
      el.scrollLeft = Math.max(0, x - view * 0.5);
    } else if (x > left + view - margin) {
      // playing → page-flip so the playhead restarts near the left;
      // a big seek → recenter
      el.scrollLeft = playing ? x - view * 0.1 : Math.max(0, x - view * 0.5);
    }
  }, [playhead, pps, playing, drag]);

  const snapStart = useCallback(
    (start: number, dur: number, movingId: string): number => {
      if (!snap) return start;
      const tol = 8 / pps;
      const cands = [0, playheadRef.current, ...markers];
      for (const c of project.clips) {
        if (c.id === movingId) continue;
        cands.push(c.start, c.start + c.len);
      }
      for (const c of cands) {
        if (Math.abs(start - c) < tol) return c;
        if (Math.abs(start + dur - c) < tol) return c - dur;
      }
      return start;
    },
    [snap, pps, markers, project]
  );

  const onRulerPointerDown = useCallback(
    (e: React.PointerEvent) => {
      capture(e);
      scrubTo(e.clientX);
    },
    [scrubTo]
  );
  const onRulerPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (e.buttons & 1) scrubTo(e.clientX);
    },
    [scrubTo]
  );

  const onClipPointerDown = useCallback(
    (e: React.PointerEvent, clip: Clip) => {
      e.stopPropagation();
      // selection: Ctrl/Cmd/Shift toggles into a multi-selection; a plain click
      // on a clip already in the selection keeps the group (so you can drag it),
      // otherwise it selects just this clip.
      const additive = e.ctrlKey || e.metaKey || e.shiftKey;
      let selNow: string[];
      setSelectedIds((prev) => {
        if (additive) selNow = prev.includes(clip.id) ? prev.filter((id) => id !== clip.id) : [...prev, clip.id];
        else selNow = prev.includes(clip.id) ? prev : [clip.id];
        return selNow;
      });
      // selecting a clip must NOT stop playback — you can click around the
      // timeline while the video keeps rolling, like Premiere / FCP
      if (additive || ctlOf(clip.track).lock) return; // modifier-click / locked: select only
      capture(e);
      const lanes = lanesRef.current!;
      const x = e.clientX - lanes.getBoundingClientRect().left;
      const localPx = x - clip.start * pps;
      const orig = { start: clip.start, len: clip.len, srcIn: clip.src_in };
      if (localPx <= TRIM_ZONE_PX) {
        setDrag({ kind: "trim-l", clipId: clip.id, ...orig, orig });
      } else if (localPx >= clip.len * pps - TRIM_ZONE_PX) {
        setDrag({ kind: "trim-r", clipId: clip.id, ...orig, orig });
      } else {
        // group move when this clip is part of a multi-selection
        const sel = selNow!;
        const group =
          sel.length > 1 && sel.includes(clip.id)
            ? project.clips
                .filter((c) => sel.includes(c.id) && !ctlOf(c.track).lock)
                .map((c) => ({ id: c.id, start: c.start, track: c.track }))
            : null;
        setDrag({
          kind: "move",
          clipId: clip.id,
          track: clip.track,
          start: clip.start,
          grabOffsetS: x / pps - clip.start,
          primaryStart: clip.start,
          group,
        });
      }
    },
    [pps, ctlOf, project.clips]
  );

  const onClipPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (!drag || !(e.buttons & 1)) return;
      const lanes = lanesRef.current!;
      const rect = lanes.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const t = x / pps;
      if (drag.kind === "move") {
        const y = e.clientY - rect.top;
        const lane = Math.min(tracks.length - 1, Math.max(0, Math.floor(y / TRACK_H)));
        const clip = project.clips.find((c) => c.id === drag.clipId);
        let start = Math.max(0, t - drag.grabOffsetS);
        start = Math.max(0, snapStart(start, clip?.len ?? 0, drag.clipId));
        // a group keeps every clip on its own track (horizontal shift only);
        // a single clip can also change track (vertical move)
        if (drag.group) {
          const delta = start - drag.primaryStart;
          const minStart = Math.min(...drag.group.map((g) => g.start));
          const clampedDelta = Math.max(-minStart, delta); // no clip past t=0
          setDrag({ ...drag, start: drag.primaryStart + clampedDelta });
        } else {
          setDrag({ ...drag, track: tracks[lane], start });
        }
        setPlayhead(start);
        return;
      }
      const { orig } = drag;
      const clip = project.clips.find((c) => c.id === drag.clipId);
      const srcLen = clip ? media[clip.media]?.duration_s ?? Infinity : Infinity;
      if (drag.kind === "trim-l") {
        const d = Math.max(-orig.srcIn, Math.min(t - orig.start, orig.len - MIN_LEN_S));
        const start = orig.start + d;
        setDrag({ ...drag, start, len: orig.len - d, srcIn: orig.srcIn + d });
        setPlayhead(start);
      } else {
        const len = Math.max(MIN_LEN_S, Math.min(t - orig.start, srcLen - orig.srcIn));
        setDrag({ ...drag, len });
        setPlayhead(orig.start + len);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [drag, project, media, pps, mode, snapStart]
  );

  const onClipPointerUp = useCallback(async () => {
    if (!drag) return;
    setDrag(null);
    const r2 = (v: number) => Math.round(v * 100) / 100;
    try {
      if (drag.kind === "move") {
        if (drag.group) {
          const delta = drag.start - drag.primaryStart;
          let snap: ProjectSnapshot | null = null;
          for (const g of drag.group) {
            snap = await moveClip(g.id, g.track, r2(Math.max(0, g.start + delta)));
          }
          if (snap) applyEdit(snap);
        } else {
          applyEdit(await moveClip(drag.clipId, drag.track, r2(drag.start)));
        }
      } else {
        applyEdit(await trimClip(drag.clipId, r2(drag.start), r2(drag.len), r2(drag.srcIn)));
      }
    } catch (e) {
      setError(String(e));
    }
  }, [drag, applyEdit]);

  // Marquee (rubber-band) select: pointer-down on empty lane space drags a box
  // and selects every clip it touches. Clips stopPropagation, so this only
  // fires on the background. Modifier held = add to the current selection.
  const onLanesPointerDown = useCallback((e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const lanes = lanesRef.current;
    if (!lanes) return;
    const rect = lanes.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;
    const additive = e.ctrlKey || e.metaKey || e.shiftKey;
    const base = additive ? [...selectedIdsRef.current] : [];
    if (!additive) setSelectedIds([]);
    let dragged = false;
    const move = (ev: PointerEvent) => {
      const cx = ev.clientX - rect.left;
      const cy = ev.clientY - rect.top;
      const box = { x: Math.min(sx, cx), y: Math.min(sy, cy), w: Math.abs(cx - sx), h: Math.abs(cy - sy) };
      if (box.w > 3 || box.h > 3) dragged = true;
      setMarquee(box);
      const hit = clipRectsRef.current
        .filter(
          (r) =>
            r.left < box.x + box.w && r.left + r.width > box.x && r.top < box.y + box.h && r.top + r.height > box.y
        )
        .map((r) => r.id);
      setSelectedIds([...new Set([...base, ...hit])]);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setMarquee(null);
      if (!dragged && !additive) setSelectedIds([]); // a plain click clears
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }, []);

  // Right-click a clip → context menu. If it isn't already selected, select it.
  const onClipContextMenu = useCallback((e: React.MouseEvent, clip: Clip) => {
    e.preventDefault();
    e.stopPropagation();
    setSelectedIds((prev) => (prev.includes(clip.id) ? prev : [clip.id]));
    setCtxMenu({ x: e.clientX, y: e.clientY, clipId: clip.id });
  }, []);

  // ── transcript editing ──────────────────────────────────────────────
  const srcToTimeline = useCallback(
    (mediaId: string, srcT: number): number | null => {
      for (const c of project.clips) {
        if (c.media !== mediaId || c.track !== "V1") continue;
        if (srcT >= c.src_in - 1e-9 && srcT < c.src_in + c.len) {
          return c.start + (srcT - c.src_in);
        }
      }
      return null;
    },
    [project]
  );

  const transcriptMedia = useMemo(() => {
    const selClip = selected ? project.clips.find((c) => c.id === selected) : null;
    if (selClip && transcripts[selClip.media]) return selClip.media;
    if (underPlayhead && transcripts[underPlayhead.media.id]) return underPlayhead.media.id;
    return Object.keys(transcripts)[0] ?? null;
  }, [selected, project, transcripts, underPlayhead]);
  const words = transcriptMedia ? transcripts[transcriptMedia] ?? null : null;

  const onWordClick = useCallback(
    (idx: number, shift: boolean) => {
      if (!transcriptMedia || !words) return;
      setWordSel((sel) =>
        shift && sel && sel.media === transcriptMedia
          ? { ...sel, b: idx }
          : { media: transcriptMedia, a: idx, b: idx }
      );
      const t = srcToTimeline(transcriptMedia, (words[idx].start + words[idx].end) / 2);
      if (t != null) setPlayhead(t);
    },
    [transcriptMedia, words, srcToTimeline]
  );

  const cutWords = useCallback(async () => {
    if (!wordSel || !words || wordSel.media !== transcriptMedia) return;
    const [i0, i1] = [Math.min(wordSel.a, wordSel.b), Math.max(wordSel.a, wordSel.b)];
    const from = words[i0].start;
    const to = words[i1].end;
    const mid = (from + to) / 2;
    const clip = project.clips.find(
      (c) => c.media === wordSel.media && c.track === "V1" && mid >= c.src_in && mid < c.src_in + c.len
    );
    if (!clip) {
      setError("No clip on V1 covers those words");
      return;
    }
    try {
      applyEdit(await razorOut(clip.id, from, to));
      setWordSel(null);
    } catch (e) {
      setError(String(e));
    }
  }, [wordSel, words, transcriptMedia, project, applyEdit]);

  const smartCuts = useMemo(() => {
    if (!transcriptMedia || !words || words.length === 0) return null;
    const alive = (r: [number, number]) => srcToTimeline(transcriptMedia, (r[0] + r[1]) / 2) !== null;
    const fillers = words
      .filter((w) => FILLERS.has(cleanWord(w.text)))
      .map((w) => [w.start, w.end] as [number, number])
      .filter(alive);
    const silences: [number, number][] = [];
    for (let i = 1; i < words.length; i++) {
      const gap = words[i].start - words[i - 1].end;
      if (gap >= SILENCE_GAP_S) {
        const r: [number, number] = [words[i - 1].end + 0.12, words[i].start - 0.12];
        if (alive(r)) silences.push(r);
      }
    }
    return { fillers, silences };
  }, [transcriptMedia, words, srcToTimeline]);

  const doCutRanges = useCallback(
    async (ranges: [number, number][]) => {
      if (!transcriptMedia || ranges.length === 0) return;
      try {
        applyEdit(await cutRanges(transcriptMedia, ranges));
        setWordSel(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [transcriptMedia, applyEdit]
  );

  // Auto-captions: turn the transcript into styled caption clips on V2.
  // Words are grouped into short lines and mapped from source time to
  // timeline time (skipping any that were cut out).
  const onGenerateCaptions = useCallback(async (mediaOverride?: string) => {
    // MediaPanel wires this as a bare click handler, so guard against an
    // event object arriving where a media id is expected.
    const mid = typeof mediaOverride === "string" ? mediaOverride : transcriptMedia;
    if (!mid) return;
    const ws = transcripts[mid];
    if (!ws || ws.length === 0) return;
    const lines: { text: string; srcStart: number; srcEnd: number }[] = [];
    let cur: Word[] = [];
    const flush = () => {
      if (cur.length === 0) return;
      lines.push({
        text: cur.map((w) => w.text.trim()).join(" ").replace(/\s+([.,!?])/g, "$1"),
        srcStart: cur[0].start,
        srcEnd: cur[cur.length - 1].end,
      });
      cur = [];
    };
    for (const w of ws) {
      cur.push(w);
      if (cur.length >= 6 || cur[cur.length - 1].end - cur[0].start >= 2.8) flush();
    }
    flush();
    const specs = lines.flatMap((l) => {
      const start = srcToTimeline(mid, l.srcStart);
      if (start === null) return [];
      const end = srcToTimeline(mid, Math.max(l.srcStart, l.srcEnd - 0.01));
      const len = Math.max(0.4, (end ?? start + (l.srcEnd - l.srcStart)) - start);
      return [{ text: l.text, start, len }];
    });
    if (specs.length === 0) {
      setError("No transcript text falls within the timeline — nothing to caption.");
      return;
    }
    try {
      // regenerating replaces any prior auto-captions (keep manual titles)
      let snap: ProjectSnapshot | null = null;
      for (const c of project.clips.filter((c) => c.text && c.name === "Caption")) {
        snap = await removeClip(c.id, false);
      }
      snap = await addCaptions(specs);
      applyEdit(snap);
    } catch (e) {
      setError(String(e));
    }
  }, [transcriptMedia, transcripts, srcToTimeline, applyEdit, project.clips]);

  const caption = useMemo(() => {
    if (!underPlayhead) return null;
    const w = transcripts[underPlayhead.media.id];
    if (!w) return null;
    const srcT = underPlayhead.srcT;
    return w.find((x) => srcT >= x.start && srcT < x.end)?.text ?? null;
  }, [underPlayhead, transcripts]);

  // the first real footage clip on the timeline — what the Create clip maker acts on
  const createClip = useMemo(
    () => project.clips.find((c) => c.media && !c.text) ?? null,
    [project.clips]
  );
  // how long the create clip's source media is (0 = unknown)
  const createDur = createClip ? media[createClip.media]?.duration_s ?? 0 : 0;

  // Auto-split: break the long clip into short-ready segments. If it's been
  // transcribed we cut on sentence boundaries near a target length; otherwise
  // we fall back to even time chunks. Each segment is a source-time range.
  const onSplitShorts = useCallback(() => {
    if (!createClip) return;
    const dur = media[createClip.media]?.duration_s || createClip.src_in + createClip.len;
    const ws = transcripts[createClip.media];
    const TARGET = 45,
      MIN = 15,
      MAX = 75;
    const segs: ShortSeg[] = [];
    if (ws && ws.length) {
      let segStart = ws[0].start;
      let startIdx = 0;
      let i = 0;
      while (i < ws.length) {
        const w = ws[i];
        const elapsed = w.end - segStart;
        const nextGap = i + 1 < ws.length ? ws[i + 1].start - w.end : 999;
        const endsSentence = /[.!?]["')]?$/.test(w.text.trim());
        const atEnd = i === ws.length - 1;
        if (atEnd || (elapsed >= TARGET && (endsSentence || nextGap > 0.45)) || elapsed >= MAX) {
          const label = ws
            .slice(startIdx, i + 1)
            .map((x) => x.text.trim())
            .join(" ")
            .replace(/\s+([.,!?])/g, "$1")
            .slice(0, 64);
          const seg = { start: segStart, end: w.end, label: label || `Clip ${segs.length + 1}` };
          // a too-short tail folds into the previous segment
          if (seg.end - seg.start < MIN && segs.length) segs[segs.length - 1].end = seg.end;
          else segs.push(seg);
          segStart = i + 1 < ws.length ? ws[i + 1].start : w.end;
          startIdx = i + 1;
        }
        i++;
      }
    } else if (dur > 0) {
      for (let t = 0; t < dur - 1; t += TARGET) {
        segs.push({ start: t, end: Math.min(dur, t + TARGET), label: `Segment ${segs.length + 1}` });
      }
    }
    setShorts(segs);
    setActiveShort(null);
  }, [createClip, media, transcripts]);

  // Strip every auto-caption from the timeline. Reads the LIVE project from the
  // backend (not a possibly-stale render snapshot) so a caption added moments
  // ago is always seen and removed. Returns the resulting snapshot (or null).
  const clearCaptions = useCallback(async (): Promise<ProjectSnapshot | null> => {
    const cur = await getProject();
    let snap: ProjectSnapshot | null = null;
    for (const c of cur.clips.filter((c) => c.text && c.name === "Caption")) {
      snap = await removeClip(c.id, false);
    }
    return snap;
  }, []);

  // Caption a picked moment. The timeline clip is now the moment [from,to] with
  // src_in=from, start=0, so a transcript word at source time `w` maps to
  // timeline time `w - from`. Words are grouped into short lines. Replaces any
  // prior auto-captions so re-picking a different moment stays clean.
  const captionMoment = useCallback(
    async (mediaId: string, from: number, to: number) => {
      const ws = transcripts[mediaId];
      const span = to - from;
      const specs: CaptionSpec[] = [];
      if (ws && ws.length) {
        let cur: Word[] = [];
        const flush = () => {
          if (!cur.length) return;
          const text = cur.map((w) => w.text.trim()).join(" ").replace(/\s+([.,!?])/g, "$1");
          const start = Math.max(0, cur[0].start - from);
          const end = Math.min(span, cur[cur.length - 1].end - from);
          if (end > start) specs.push({ text, start, len: Math.max(0.4, end - start) });
          cur = [];
        };
        for (const w of ws) {
          if (w.end <= from || w.start >= to) continue; // outside the moment
          cur.push(w);
          if (cur.length >= 6 || cur[cur.length - 1].end - cur[0].start >= 2.8) flush();
        }
        flush();
      }
      try {
        // replace any prior captions with the new set (live-project read)
        let snap = await clearCaptions();
        if (specs.length) snap = await addCaptions(specs);
        if (snap) applyEdit(snap);
      } catch (e) {
        setError(String(e));
      }
    },
    [transcripts, clearCaptions, applyEdit]
  );

  // Load a chosen moment: retrim the timeline clip to that source range (so the
  // whole timeline *is* that short and the normal Export renders just it). Only
  // burns captions onto it when the Captions toggle is on — never automatically.
  const onPickShort = useCallback(
    async (i: number) => {
      if (!createClip) return;
      const s = shorts[i];
      if (!s) return;
      setActiveShort(i);
      try {
        applyEdit(await trimClip(createClip.id, 0, s.end - s.start, s.start));
        setPlayhead(0);
        if (captionsOn) await captionMoment(createClip.media, s.start, s.end);
      } catch (e) {
        setError(String(e));
      }
    },
    [createClip, shorts, applyEdit, captionMoment, captionsOn]
  );

  // Toggle captions on the loaded clip. Turning on captions the clip's current
  // source range; turning off strips the auto-caption clips. Persisted so the
  // choice sticks — captions never get added unless you ask.
  const onToggleCaptions = useCallback(
    (on: boolean) => {
      setCaptionsOn(on);
      localStorage.setItem("cutlass-captions-on", on ? "1" : "0");
      savePref("captionsOn", on).catch(() => {});
      if (on) {
        if (createClip) {
          captionMoment(createClip.media, createClip.src_in, createClip.src_in + createClip.len);
        }
      } else {
        // turning off ALWAYS strips captions (no clip guard) — live-project read
        clearCaptions()
          .then((snap) => snap && applyEdit(snap))
          .catch((e) => setError(String(e)));
      }
    },
    [createClip, captionMoment, clearCaptions, applyEdit]
  );

  // Run the AI moment-finder on a media's transcript: Claude reads the whole
  // transcript and returns the funniest / most exciting / pivotal standout
  // moments. Populates the shorts list. Requires a transcript already in state.
  const runAi = useCallback(
    async (mediaId: string) => {
      const ws = transcripts[mediaId];
      if (!ws || ws.length < 5) {
        setError("This clip has too little speech for the AI to find moments. Try a clip with more dialogue.");
        return;
      }
      setAiFinding(true);
      setError(null);
      try {
        const moments = await aiHighlights(ws, 8);
        if (moments.length === 0) {
          setError("The AI didn't find clear standout moments here. Try a longer or livelier clip.");
          return;
        }
        setShorts(
          moments.map((m) => ({
            start: m.start,
            end: m.end,
            label: m.title,
            reasons: m.reason ? [m.reason] : [],
          }))
        );
        setActiveShort(null);
      } catch (e) {
        setError(`Couldn't find moments: ${String(e)}`);
      } finally {
        setAiFinding(false);
        refreshUsage();
      }
    },
    [transcripts, refreshUsage]
  );

  // ONE button. Transcribe the clip on-device if needed (progress shows on the
  // button), then hand the transcript to the AI to find the best moments. The
  // two steps are chained through `wantAi` since the transcript lands in state
  // asynchronously — the effect below fires the AI pass once it arrives.
  const onFindHighlights = useCallback(() => {
    if (!createClip) return;
    const mid = createClip.media;
    if (transcripts[mid]) {
      runAi(mid);
      return;
    }
    setWantAi(mid);
    if (transcribing !== mid) doTranscribe(mid, transcribeMode === "fast");
  }, [createClip, transcripts, runAi, transcribing, doTranscribe, transcribeMode]);

  useEffect(() => {
    if (wantAi && transcripts[wantAi]) {
      const mid = wantAi;
      setWantAi(null);
      runAi(mid);
    }
  }, [wantAi, transcripts, runAi]);

  // stale shorts belong to whatever clip was loaded before — drop them when
  // the source media changes (or the clip goes away)
  const createMediaId = createClip?.media ?? null;
  useEffect(() => {
    setShorts([]);
    setActiveShort(null);
  }, [createMediaId]);

  // ── layout ──────────────────────────────────────────────────────────
  const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

  return (
    <div className="app">
      <TopBar
        projectName={project.name}
        dirty={dirty}
        mode={mode}
        room={room}
        exporting={exportModal?.phase === "running" ? exportModal.progress : null}
        canEdit={clips.length > 0}
        inTauri={inTauri}
        onMode={switchMode}
        onImport={doImport}
        onOpen={doOpen}
        onSave={doSave}
        onSaveAs={doSaveAs}
        autoSave={autoSave}
        onToggleAutoSave={toggleAutoSave}
        onExport={doExport}
        onUndo={doUndo}
        onRedo={doRedo}
        onSplit={doSplit}
        onAddTitle={onAddTitle}
        onDeleteSel={doDeleteSel}
        onCollab={doCollab}
        onZoom={(dir) =>
          setPps((z) => clamp(z * (dir > 0 ? 1.25 : 0.8), PPS_MIN, PPS_MAX))
        }
        hasSelection={selected !== null}
        onFeedback={() => setFeedbackOpen(true)}
      />

      {busy && <div className="notice">{busy}</div>}
      {transcribing && mode === "create" && (
        <div className="notice">Transcribing on-device — words will appear shortly…</div>
      )}
      {error && (
        <div className="notice error" onClick={() => setError(null)}>
          {error} <span className="dismiss">dismiss</span>
        </div>
      )}

      <main className="workspace">
        {/* the media bin is where imports land — shown in both modes so
            there's always something to drag onto the timeline */}
        <div
          className={mode === "create" ? "left-col create-left" : "left-col"}
          style={{ width: leftW, display: "flex", flexDirection: "column", minWidth: 0 }}
        >
          {mode === "create" && (
            <ClipFormat
              format={clipFormat}
              onFormat={setClipFormat}
              reframe={clipReframe}
              onReframe={setClipReframe}
              reframeX={clipReframeX}
              onReframeX={setClipReframeX}
              hasClip={createClip !== null}
              transcribing={createClip !== null && transcribing === createClip.media}
              transcribePct={createClip ? transcribeProgress[createClip.media] ?? 0 : 0}
              aiFinding={aiFinding}
              transcribeMode={transcribeMode}
              onTranscribeMode={chooseTranscribeMode}
              captionsOn={captionsOn}
              onToggleCaptions={onToggleCaptions}
              usage={usage}
              onFindHighlights={onFindHighlights}
              exporting={exportModal?.phase === "running"}
              onExport={exportClip}
              canSplit={createClip !== null && createDur > 75}
              shorts={shorts}
              activeShort={activeShort}
              onSplit={onSplitShorts}
              onPickShort={onPickShort}
            />
          )}
          <MediaPanel
            media={Object.values(media)}
            transcripts={transcripts}
            transcribing={transcribing}
            transcribeProgress={transcribeProgress}
            transcribeMode={transcribeMode}
            onTranscribeMode={chooseTranscribeMode}
            onImport={doImport}
            onTranscribe={(id) => doTranscribe(id, transcribeMode === "fast")}
            onMediaPointerDown={onMediaPointerDown}
            onRemoveMedia={onRemoveMedia}
            hasSelection={selected !== null}
            onApplyEffect={onApplyEffect}
            onToggleEffect={onToggleEffect}
            effectActive={effectActive}
            onKenBurns={onKenBurns}
            captionsReady={transcriptMedia !== null}
            onGenerateCaptions={onGenerateCaptions}
            frameSrc={underPlayhead ? thumbAt(underPlayhead.media, underPlayhead.srcT) : null}
            customLooks={customLooks}
            onSaveLook={onSaveLook}
            onDeleteLook={onDeleteLook}
            selectedLut={selectedClip?.lut || ""}
            onImportLut={onImportLut}
            onRemoveLut={onRemoveLut}
            busy={busy !== null}
          />
        </div>
        <Resizer direction="h" onDelta={(d) => setLeftW((w) => clamp(w + d, 200, 440))} />

        <Monitor
          layers={monitorLayers}
          vignette={monitorOverlay.vignette}
          grain={monitorOverlay.grain}
          censors={monitorCensors}
          onCensor={onCensor}
          format={
            mode === "create"
              ? { w: clipFormat.w, h: clipFormat.h, reframe: clipReframe, rx: clipReframeX }
              : null
          }
          titleOverlay={titleOverlay}
          caption={caption}
          playhead={playhead}
          playing={playing}
          canPlay={clips.length > 0}
          resolution={resolution}
          onResolution={setResolution}
          onTogglePlay={() => setPlaying((p) => !p)}
          onSeek={setPlayhead}
          contentEnd={contentEndS}
        />

        <Resizer direction="h" onDelta={(d) => setRightW((w) => clamp(w - d, 240, 480))} />
        <div style={{ width: rightW, display: "flex", minWidth: 0 }}>
          <Inspector
            mode={mode}
            clip={selectedClip}
            media={media}
            onMove={(id, track, start) =>
              moveClip(id, track, Math.max(0, start)).then(applyEdit).catch((e) => setError(String(e)))
            }
            onTrim={(id, start, len, srcIn) =>
              trimClip(id, start, len, srcIn).then(applyEdit).catch((e) => setError(String(e)))
            }
            onFxPreview={onFxPreview}
            onFxCommit={onFxCommit}
            clipTime={clipTime}
            onSetKeyframe={onSetKeyframe}
            onClearKeyframes={onClearKeyframes}
            onTrackCensor={onTrackCensor}
            onRemoveCensor={onRemoveCensor}
            tracking={tracking}
            trackProgress={trackProgress}
            hasLeftNeighbor={selectedHasLeftNeighbor}
            onSetTransition={onSetTransition}
            onSetTitleText={onSetTitleText}
            onPreviewTitleText={setTextDraft}
            words={words}
            transcriptMediaName={transcriptMedia ? media[transcriptMedia]?.name ?? null : null}
            wordSel={wordSel && wordSel.media === transcriptMedia ? wordSel : null}
            isWordCut={(w) =>
              transcriptMedia ? srcToTimeline(transcriptMedia, (w.start + w.end) / 2) === null : false
            }
            isFiller={(w) => FILLERS.has(cleanWord(w.text))}
            onWordClick={onWordClick}
            onCutSelection={cutWords}
            smartCuts={smartCuts}
            onCutRanges={doCutRanges}
            silenceGapS={SILENCE_GAP_S}
          />
        </div>
      </main>

      <Resizer direction="v" onDelta={(d) => setTlH((h) => clamp(h - d, 180, 760))} />
      <Timeline
        tracks={tracks}
        clips={clips}
        media={media}
        pps={pps}
        onPps={setPps}
        playhead={playhead}
        peers={Object.values(peers)}
        selectedIds={selectedIds}
        dragClipId={drag?.clipId ?? null}
        marquee={marquee}
        timelineEndS={timelineEndS}
        snap={snap}
        onSnap={setSnap}
        markers={markers}
        trackCtl={trackCtl}
        onTrackCtl={setTrack}
        height={tlH}
        lanesRef={lanesRef}
        scrollRef={scrollRef}
        onRulerPointerDown={onRulerPointerDown}
        onRulerPointerMove={onRulerPointerMove}
        onLanesPointerDown={onLanesPointerDown}
        onClipPointerDown={onClipPointerDown}
        onClipContextMenu={onClipContextMenu}
        onClipPointerMove={onClipPointerMove}
        onClipPointerUp={onClipPointerUp}
        onSeek={setPlayhead}
        showTrackHeads={mode === "studio"}
        onAddVideoTrack={addVideoTrack}
        onAddAudioTrack={addAudioTrack}
        onRemoveTrack={onRemoveTrack}
        canAddVideo={vCount < MAX_TRACKS}
        canAddAudio={aCount < MAX_TRACKS}
        dropActive={mediaGhost !== null}
      />
      {mediaGhost && (
        <div className="media-ghost" style={{ left: mediaGhost.x + 12, top: mediaGhost.y + 12 }}>
          {mediaGhost.name}
        </div>
      )}

      {ctxMenu && (
        <div
          className="ctx-overlay"
          onPointerDown={() => setCtxMenu(null)}
          onContextMenu={(e) => {
            e.preventDefault();
            setCtxMenu(null);
          }}
        >
          <div
            className="ctx-menu"
            style={{ left: ctxMenu.x, top: ctxMenu.y }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            <button
              className="ctx-item danger"
              onClick={() => {
                doDeleteSel(false);
                setCtxMenu(null);
              }}
            >
              Delete{selectedIds.length > 1 ? ` ${selectedIds.length} clips` : ""}
            </button>
            <button
              className="ctx-item"
              onClick={() => {
                doDeleteSel(true);
                setCtxMenu(null);
              }}
            >
              Ripple delete{selectedIds.length > 1 ? ` ${selectedIds.length} clips` : ""}
            </button>
            <button
              className="ctx-item"
              onClick={() => {
                doSplit();
                setCtxMenu(null);
              }}
            >
              Split at playhead
            </button>
            <div className="ctx-sep" />
            <button
              className="ctx-item"
              onClick={() => {
                setSelectedIds(clipIdsRef.current);
                setCtxMenu(null);
              }}
            >
              Select all
            </button>
            <button
              className="ctx-item"
              onClick={() => {
                setSelectedIds([]);
                setCtxMenu(null);
              }}
            >
              Deselect
            </button>
          </div>
        </div>
      )}

      {exportOpen && (
        <ExportDialog
          initialDir={exportDir}
          sourceHeight={sourceHeight}
          onCancel={() => setExportOpen(false)}
          onExport={runExport}
        />
      )}

      {exportModal && (
        <div className="modal-overlay">
          <div className="modal export-progress" onPointerDown={(e) => e.stopPropagation()}>
            {exportModal.phase === "running" && (
              <>
                <div className="modal-title">
                  {exportModal.cancelling ? "Cancelling…" : `Exporting ${exportModal.name}`}
                </div>
                <div className="progress-track">
                  <div
                    className="progress-fill"
                    style={{ width: `${Math.round(exportModal.progress * 100)}%` }}
                  />
                </div>
                <div className="progress-pct">{Math.round(exportModal.progress * 100)}%</div>
                <div className="modal-actions">
                  <button
                    className="ghost-btn"
                    onClick={onCancelExport}
                    disabled={exportModal.cancelling}
                  >
                    {exportModal.cancelling ? "Cancelling…" : "Cancel"}
                  </button>
                </div>
              </>
            )}
            {exportModal.phase === "done" && (
              <>
                <div className="modal-title">Export complete ✓</div>
                <div className="export-path" title={exportModal.path}>
                  {exportModal.path}
                </div>
                <div className="modal-sub">Encoded with {exportModal.encoder}</div>
                <div className="modal-actions">
                  <button className="ghost-btn" onClick={() => setExportModal(null)}>
                    Close
                  </button>
                  <button className="primary-action" onClick={() => revealFile(exportModal.path)}>
                    Open folder
                  </button>
                </div>
              </>
            )}
            {exportModal.phase === "error" && (
              <>
                <div className="modal-title">Export failed</div>
                <div className="modal-sub error">{exportModal.message}</div>
                <div className="modal-actions">
                  <button className="primary-action" onClick={() => setExportModal(null)}>
                    Close
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}

      {feedbackOpen && (
        <div className="modal-overlay" onPointerDown={() => setFeedbackOpen(false)}>
          <div className="modal" onPointerDown={(e) => e.stopPropagation()}>
            <div className="modal-title">Send beta feedback</div>
            <div className="modal-sub">
              What worked, what broke, what you wish it did? Opens your mail app.
            </div>
            <textarea
              className="feedback-text"
              autoFocus
              rows={5}
              placeholder="Your feedback…"
              value={feedbackText}
              onChange={(e) => setFeedbackText(e.target.value)}
            />
            <div className="modal-actions">
              <button className="ghost-btn" onClick={() => setFeedbackOpen(false)}>
                Cancel
              </button>
              <button
                className="primary-action"
                disabled={!feedbackText.trim()}
                onClick={onSendFeedback}
              >
                Send
              </button>
            </div>
          </div>
        </div>
      )}

      {quitPromptOpen && (
        <div className="modal-overlay" onPointerDown={() => setQuitPromptOpen(false)}>
          <div className="modal" onPointerDown={(e) => e.stopPropagation()}>
            <div className="modal-title">Save changes before closing?</div>
            <div className="modal-sub">
              You have unsaved changes in “{project.name}”. Save them before Cutlass closes?
            </div>
            <div className="modal-actions">
              <button className="ghost-btn" onClick={() => setQuitPromptOpen(false)}>
                Cancel
              </button>
              <button className="ghost-btn danger" onClick={onQuitDiscard}>
                Don’t save
              </button>
              <button className="primary-action" onClick={onQuitSave}>
                Save
              </button>
            </div>
          </div>
        </div>
      )}

      {removeMediaAsk && (
        <div className="modal-overlay" onPointerDown={() => setRemoveMediaAsk(null)}>
          <div className="modal" onPointerDown={(e) => e.stopPropagation()}>
            <div className="modal-title">Remove media?</div>
            <div className="modal-sub">
              “{removeMediaAsk.name}” is used by {removeMediaAsk.clips}{" "}
              {removeMediaAsk.clips === 1 ? "clip" : "clips"} on the timeline. Removing it will
              delete {removeMediaAsk.clips === 1 ? "that clip" : "those clips"} too.
            </div>
            <div className="modal-actions">
              <button className="ghost-btn" onClick={() => setRemoveMediaAsk(null)}>
                Cancel
              </button>
              <button
                className="ghost-btn danger"
                onClick={() => {
                  const id = removeMediaAsk.id;
                  setRemoveMediaAsk(null);
                  doRemoveMedia(id);
                }}
              >
                Remove
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
