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
  audioClock,
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
  getProject,
  hydrateMedia,
  importMedia,
  inTauri,
  joinSession,
  moveClip,
  onExportProgress,
  onPlaybackFrame,
  onPresence,
  onProjectChanged,
  openProject,
  takeStartupFile,
  onOpenFile,
  loadPrefs,
  savePref,
  openUrl,
  pauseAudio,
  pickVideo,
  playAudio,
  razorOut,
  redoEdit,
  removeClip,
  removeTrack,
  revealFile,
  saveProject,
  splitClip,
  sendPresence,
  setEffect,
  setEffects,
  setKeyframe,
  setTitleText,
  setTransition,
  trackIndex,
  trackKind,
  transcribeMedia,
  trimClip,
  undoEdit,
} from "./ipc";
import { Mode, TopBar } from "./components/TopBar";
import { MediaPanel } from "./components/MediaPanel";
import { Monitor, RESOLUTIONS, Resolution } from "./components/Monitor";
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
  | { kind: "move"; clipId: string; track: string; start: number; grabOffsetS: number }
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
  const [selected, setSelected] = useState<string | null>(null);
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
  const [playing, setPlaying] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcripts, setTranscripts] = useState<Record<string, Word[]>>({});
  const [transcribing, setTranscribing] = useState<string | null>(null);
  const [wordSel, setWordSel] = useState<{ media: string; a: number; b: number } | null>(null);
  const [room, setRoom] = useState<string | null>(null);
  const [feedbackOpen, setFeedbackOpen] = useState(false);
  const [feedbackText, setFeedbackText] = useState("");
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

  const lanesRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  const switchMode = useCallback((m: Mode) => {
    setMode(m);
    localStorage.setItem("cutlass-mode", m);
  }, []);
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
    getProject().then(setProject).catch((e) => setError(String(e)));
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
      unFrame.then((f) => f());
      unPresence.then((f) => f());
      clearInterval(prune);
    };
  }, []);

  // ── derived clip state (drag overrides applied) ─────────────────────
  const clips = useMemo(() => {
    if (!drag) return project.clips;
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

  // ── playback (audio owns the clock when live) ───────────────────────
  const playheadRef = useRef(0);
  useEffect(() => {
    playheadRef.current = playhead;
  }, [playhead]);
  const playingRef = useRef(false);
  useEffect(() => {
    playingRef.current = playing;
  }, [playing]);
  // latest tracks / zoom / media for the pointer-drag drop closure
  const tracksRef = useRef(tracks);
  useEffect(() => {
    tracksRef.current = tracks;
  }, [tracks]);
  const ppsRef = useRef(pps);
  useEffect(() => {
    ppsRef.current = pps;
  }, [pps]);
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
  }, [playing]);

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

  const onAddTitle = useCallback(() => {
    addTitle(playheadRef.current)
      .then((snap) => {
        applyEdit(snap);
        // select the newest title so the editor opens
        const t = snap.clips.filter((c) => c.text).slice(-1)[0];
        if (t) setSelected(t.id);
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
  const doTranscribe = useCallback(async (mediaId: string) => {
    setTranscribing(mediaId);
    try {
      const words = await transcribeMedia(mediaId);
      setTranscripts((t) => ({ ...t, [mediaId]: words }));
      setDirty(true); // transcript is now stored in the doc → savable

    } catch (e) {
      setError(String(e));
    } finally {
      setTranscribing(null);
    }
  }, []);

  const doImport = useCallback(async () => {
    setError(null);
    const path = await pickVideo();
    if (!path) return;
    setBusy(`Importing ${path.split(/[\\/]/).pop()}…`);
    try {
      const res = await importMedia(path);
      setMedia((m) => ({ ...m, [res.media.id]: res.media }));
      applyEdit(res.project);
      // The clip always waits in the bin; you drag it onto a track when
      // you want it. Create mode still auto-transcribes for the workflow.
      if (mode === "create") doTranscribe(res.media.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

  // place a clip from a bin item at a given track + start
  const onDropMedia = useCallback(
    async (mediaId: string, track: string, start: number) => {
      try {
        const placed = await addClipFromMedia(mediaId, track, start);
        applyEdit(placed.project);
        setSelected(placed.clipId);
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
        const start = Math.max(0, (ev.clientX - rect.left) / ppsRef.current);
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
      setSelected(null);
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
      if (!selected) return;
      removeClip(selected, ripple).then(applyEdit).catch((e) => setError(String(e)));
      setSelected(null);
    },
    [selected, applyEdit]
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
      } else if ((e.key === "Delete" || e.key === "Backspace") && selected) {
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
      setSelected(clip.id);
      // selecting a clip must NOT stop playback — you can click around the
      // timeline while the video keeps rolling, like Premiere / FCP
      if (ctlOf(clip.track).lock) return; // locked: select only
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
        setDrag({
          kind: "move",
          clipId: clip.id,
          track: clip.track,
          start: clip.start,
          grabOffsetS: x / pps - clip.start,
        });
      }
    },
    [pps, ctlOf]
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
        setDrag({ ...drag, track: tracks[lane], start });
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
        applyEdit(await moveClip(drag.clipId, drag.track, r2(drag.start)));
      } else {
        applyEdit(await trimClip(drag.clipId, r2(drag.start), r2(drag.len), r2(drag.srcIn)));
      }
    } catch (e) {
      setError(String(e));
    }
  }, [drag, applyEdit]);

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
  const onGenerateCaptions = useCallback(async () => {
    if (!transcriptMedia) return;
    const ws = transcripts[transcriptMedia];
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
      const start = srcToTimeline(transcriptMedia, l.srcStart);
      if (start === null) return [];
      const end = srcToTimeline(transcriptMedia, Math.max(l.srcStart, l.srcEnd - 0.01));
      const len = Math.max(0.4, (end ?? start + (l.srcEnd - l.srcStart)) - start);
      return [{ text: l.text, start, len }];
    });
    if (specs.length === 0) {
      setError("No transcript text falls within the timeline — nothing to caption.");
      return;
    }
    try {
      applyEdit(await addCaptions(specs));
    } catch (e) {
      setError(String(e));
    }
  }, [transcriptMedia, transcripts, srcToTimeline, applyEdit]);

  const caption = useMemo(() => {
    if (!underPlayhead) return null;
    const w = transcripts[underPlayhead.media.id];
    if (!w) return null;
    const srcT = underPlayhead.srcT;
    return w.find((x) => srcT >= x.start && srcT < x.end)?.text ?? null;
  }, [underPlayhead, transcripts]);

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
        <div style={{ width: leftW, display: "flex", minWidth: 0 }}>
          <MediaPanel
            media={Object.values(media)}
            transcripts={transcripts}
            transcribing={transcribing}
            onImport={doImport}
            onTranscribe={doTranscribe}
            onMediaPointerDown={onMediaPointerDown}
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
        selected={selected}
        dragClipId={drag?.clipId ?? null}
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
        onLanesPointerDown={() => setSelected(null)}
        onClipPointerDown={onClipPointerDown}
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
    </div>
  );
}
