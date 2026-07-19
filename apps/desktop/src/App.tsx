import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Clip,
  MediaItem,
  Presence,
  ProjectSnapshot,
  Word,
  addTitle,
  audioClock,
  clearKeyframes,
  currentRoom,
  cutRanges,
  exactFrame,
  exportProject,
  getProject,
  hydrateMedia,
  importMedia,
  inTauri,
  joinSession,
  moveClip,
  onExportProgress,
  onPresence,
  onProjectChanged,
  openProject,
  pauseAudio,
  pickVideo,
  playAudio,
  razorOut,
  redoEdit,
  removeClip,
  saveProject,
  sendPresence,
  setEffect,
  setKeyframe,
  setTitleText,
  setTransition,
  transcribeMedia,
  trimClip,
  undoEdit,
} from "./ipc";
import { Mode, TopBar } from "./components/TopBar";
import { MediaPanel } from "./components/MediaPanel";
import { Monitor, RESOLUTIONS, Resolution } from "./components/Monitor";
import { Inspector } from "./components/Inspector";
import { PPS_MAX, PPS_MIN, TRACK_H, Timeline, TrackCtl } from "./components/Timeline";
import { fxStyle, Resizer } from "./components/ui";

const TRACKS = ["V2", "V1"]; // rendered top→bottom; V2 wins for preview
const TRIM_ZONE_PX = 10;
const MIN_LEN_S = 0.1;
const PPS_DEFAULT = 48;
const FILLERS = new Set(["um", "uh", "uhh", "umm", "erm", "er", "ah", "hmm", "mm", "mhm"]);
const cleanWord = (t: string) => t.toLowerCase().replace(/[^a-z']/g, "");
const SILENCE_GAP_S = 0.8;

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
  const [playhead, setPlayhead] = useState(0);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcripts, setTranscripts] = useState<Record<string, Word[]>>({});
  const [transcribing, setTranscribing] = useState<string | null>(null);
  const [wordSel, setWordSel] = useState<{ media: string; a: number; b: number } | null>(null);
  const [room, setRoom] = useState<string | null>(null);
  const [exporting, setExporting] = useState<number | null>(null);
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
  const tracks = mode === "create" ? ["V1"] : TRACKS;
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

  // ── presence identity ───────────────────────────────────────────────
  const me = useRef({ id: Math.random().toString(36).slice(2, 8), name: "", color: "" });
  if (!me.current.name) {
    me.current.name = `editor-${me.current.id.slice(0, 4)}`;
    me.current.color = `hsl(${((parseInt(me.current.id, 36) % 360) + 360) % 360}, 75%, 60%)`;
  }

  // ── mount: load project, subscribe to events ────────────────────────
  useEffect(() => {
    getProject().then(setProject).catch((e) => setError(String(e)));
    currentRoom().then((r) => r && setRoom(r));
    const un = onProjectChanged((snap) => setProject(snap));
    const unExport = onExportProgress((p) => setExporting(p));
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

  // ── program monitor source ──────────────────────────────────────────
  const underPlayhead = useMemo(() => {
    for (const track of tracks) {
      if (ctlOf(track).hide) continue;
      const clip = clips.find(
        (c) => c.track === track && playhead >= c.start && playhead < c.start + c.len
      );
      if (!clip) continue;
      const m = media[clip.media];
      if (!m || m.thumbs.length === 0) continue;
      const speed = clip.fx?.speed ?? 1;
      const srcT = clip.src_in + (playhead - clip.start) * speed;
      return { media: m, srcT: Math.min(srcT, m.duration_s), clip };
    }
    return null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clips, media, playhead, mode, trackCtl]);

  const proxySrc = useMemo(() => {
    if (!underPlayhead) return null;
    const { media: m, srcT } = underPlayhead;
    const idx = Math.min(m.thumbs.length - 1, Math.max(0, Math.floor(srcT * m.scrub_fps)));
    return m.thumbs[idx];
  }, [underPlayhead]);

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
  const previewSrc = hq && hq.key === hqKey ? hq.src : proxySrc;

  // ── effects: live preview draft for the selected clip ───────────────
  const [fxDraft, setFxDraft] = useState<Record<string, number>>({});
  useEffect(() => setFxDraft({}), [selected]); // reset when selection changes

  const previewStyle = useMemo(() => {
    const clip = underPlayhead?.clip;
    if (!clip) return {};
    const draftApplies = clip.id === selected && Object.keys(fxDraft).length > 0;
    const effClip = draftApplies
      ? { ...clip, fx: { ...(clip.fx ?? {}), ...fxDraft } }
      : clip;
    return fxStyle(effClip, playhead);
  }, [underPlayhead, selected, fxDraft, playhead]);

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
      setTitleText(selected, text).then(applyEdit).catch((e) => setError(String(e)));
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
      const fs = (c.fx?.font_size ?? 56) / 1080; // fraction of frame height
      const bg = c.fx?.title_bg ?? 0;
      const px = (c.fx?.pos_x ?? 0) * 100;
      const py = (c.fx?.pos_y ?? 0) * 100;
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
            {c.text}
          </span>
        </div>
      );
    });
  }, [clips, playhead]);

  // ── import / transcribe ─────────────────────────────────────────────
  const doTranscribe = useCallback(async (mediaId: string) => {
    setTranscribing(mediaId);
    try {
      const words = await transcribeMedia(mediaId);
      setTranscripts((t) => ({ ...t, [mediaId]: words }));
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
      if (mode === "create") doTranscribe(res.media.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode]);

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
  const doSave = useCallback(async () => {
    try {
      if (await saveProject()) setDirty(false);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const doOpen = useCallback(async () => {
    try {
      const res = await openProject();
      if (!res) return;
      setProject(res.project);
      const map: Record<string, MediaItem> = {};
      for (const m of res.media) map[m.id] = m;
      setMedia(map);
      setSelected(null);
      setWordSel(null);
      setTranscripts({});
      setPlayhead(0);
      setDirty(false);
      setMarkers([]);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const doExport = useCallback(async () => {
    setError(null);
    setExporting(0);
    try {
      const encoder = await exportProject(resolution.w, resolution.h);
      setExporting(null);
      if (encoder) {
        setBusy(`Exported ✓ ${resolution.label} (${encoder})`);
        setTimeout(() => setBusy(null), 4000);
      }
    } catch (e) {
      setExporting(null);
      setError(String(e));
    }
  }, [resolution]);

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

  // ── keyboard ────────────────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement)?.tagName === "INPUT") return;
      if (e.ctrlKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        e.shiftKey ? doRedo() : doUndo();
      } else if (e.ctrlKey && e.key.toLowerCase() === "y") {
        e.preventDefault();
        doRedo();
      } else if (e.ctrlKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        doSave();
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
  }, [selected, doSave, doOpen, doUndo, doRedo, doDeleteSel]);

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
      setPlaying(false);
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
        exporting={exporting}
        canEdit={clips.length > 0}
        inTauri={inTauri}
        onMode={switchMode}
        onImport={doImport}
        onOpen={doOpen}
        onSave={doSave}
        onExport={doExport}
        onUndo={doUndo}
        onRedo={doRedo}
        onDeleteSel={doDeleteSel}
        onCollab={doCollab}
        onZoom={(dir) =>
          setPps((z) => clamp(z * (dir > 0 ? 1.25 : 0.8), PPS_MIN, PPS_MAX))
        }
        hasSelection={selected !== null}
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
        {mode === "studio" && (
          <>
            <div style={{ width: leftW, display: "flex", minWidth: 0 }}>
              <MediaPanel
                media={Object.values(media)}
                transcripts={transcripts}
                transcribing={transcribing}
                onImport={doImport}
                onAddTitle={onAddTitle}
                onTranscribe={doTranscribe}
                busy={busy !== null}
              />
            </div>
            <Resizer direction="h" onDelta={(d) => setLeftW((w) => clamp(w + d, 200, 440))} />
          </>
        )}

        <Monitor
          src={previewSrc}
          imgStyle={previewStyle}
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

      <Resizer direction="v" onDelta={(d) => setTlH((h) => clamp(h - d, 180, 520))} />
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
      />
    </div>
  );
}
