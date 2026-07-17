import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Clip,
  MediaItem,
  ProjectSnapshot,
  audioClock,
  exactFrame,
  exportProject,
  getProject,
  hydrateMedia,
  importMedia,
  inTauri,
  joinSession,
  cutRanges,
  moveClip,
  onExportProgress,
  onProjectChanged,
  openProject,
  saveProject,
  pauseAudio,
  pickVideo,
  playAudio,
  razorOut,
  redoEdit,
  removeClip,
  transcribeMedia,
  trimClip,
  undoEdit,
  Word,
} from "./ipc";

const FILLERS = new Set(["um", "uh", "uhh", "umm", "erm", "er", "ah", "hmm", "mm", "mhm"]);
const cleanWord = (t: string) => t.toLowerCase().replace(/[^a-z']/g, "");
const SILENCE_GAP_S = 0.8;

// timeline pixels per second (zoomable)
const PPS_DEFAULT = 48;
const PPS_MIN = 12;
const PPS_MAX = 160;

// pointer capture keeps drags alive outside the element, but its failure
// (synthetic events, released pointers) must never kill the interaction
function capture(e: React.PointerEvent) {
  try {
    e.currentTarget.setPointerCapture(e.pointerId);
  } catch {
    /* drag still works while the pointer stays over the element */
  }
}
const TRACKS = ["V2", "V1"]; // rendered top→bottom; V2 wins for preview
const TRACK_H = 64;

const TRIM_ZONE_PX = 10; // clip-edge width that grabs as a trim handle
const MIN_LEN_S = 0.1;

type DragState =
  | {
      kind: "move";
      clipId: string;
      track: string;
      start: number;
      grabOffsetS: number; // pointer offset into the clip, seconds
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
  const [project, setProject] = useState<ProjectSnapshot>({ name: "Untitled", clips: [] });
  const [media, setMedia] = useState<Record<string, MediaItem>>({});
  const [playhead, setPlayhead] = useState(0);
  const [pps, setPps] = useState(PPS_DEFAULT);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcripts, setTranscripts] = useState<Record<string, Word[]>>({});
  const [transcribing, setTranscribing] = useState<string | null>(null);
  const [wordSel, setWordSel] = useState<{ media: string; a: number; b: number } | null>(null);
  const lanesRef = useRef<HTMLDivElement>(null);

  const [room, setRoom] = useState<string | null>(null);

  const [exporting, setExporting] = useState<number | null>(null);

  useEffect(() => {
    getProject().then(setProject).catch((e) => setError(String(e)));
    // remote collab edits land here
    const un = onProjectChanged((snap) => setProject(snap));
    const unExport = onExportProgress((p) => setExporting(p));
    return () => {
      un.then((f) => f());
      unExport.then((f) => f());
    };
  }, []);

  const doExport = useCallback(async () => {
    setError(null);
    setExporting(0);
    try {
      const encoder = await exportProject();
      setExporting(null);
      if (encoder) setBusy(`Exported ✓ (${encoder})`);
      setTimeout(() => setBusy(null), 4000);
    } catch (e) {
      setExporting(null);
      setError(String(e));
    }
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

  // ── playback ────────────────────────────────────────────────────────
  // The native audio player owns the transport clock when it's running:
  // a local interval clock (not rAF — rAF freezes in hidden windows)
  // animates the playhead smoothly, and re-syncs to the audio clock every
  // 250 ms. Without audio (mock mode / no device) the local clock rules.
  const playheadRef = useRef(0);
  useEffect(() => {
    playheadRef.current = playhead;
  }, [playhead]);
  const contentEndRef = useRef(0);
  useEffect(() => {
    contentEndRef.current = contentEndS;
  }, [contentEndS]);

  useEffect(() => {
    if (!playing) return;
    let cancelled = false;
    let audioLive = false;
    playAudio(playheadRef.current).then((ok) => {
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
        if (t != null) setPlayhead(t); // land exactly where the audio stopped
      });
    };
  }, [playing]);

  // ── save / open ─────────────────────────────────────────────────────
  const doSave = useCallback(async () => {
    try {
      if (await saveProject()) setBusy(null);
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
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // hydrate media referenced by the project but not local (opened project
  // or collab peer) — the doc carries name/path/duration for each id
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

  // ── keyboard: space, delete, ctrl+s / ctrl+o ────────────────────────
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        (e.shiftKey ? redoEdit() : undoEdit())
          .then(setProject)
          .catch((err) => setError(String(err)));
      } else if (e.ctrlKey && e.key.toLowerCase() === "y") {
        e.preventDefault();
        redoEdit().then(setProject).catch((err) => setError(String(err)));
      } else if (e.ctrlKey && e.key.toLowerCase() === "s") {
        e.preventDefault();
        doSave();
      } else if (e.ctrlKey && e.key.toLowerCase() === "o") {
        e.preventDefault();
        doOpen();
      } else if (e.code === "Space") {
        e.preventDefault();
        setPlaying((p) => !p);
      } else if ((e.key === "Delete" || e.key === "Backspace") && selected) {
        e.preventDefault();
        removeClip(selected, e.shiftKey)
          .then(setProject)
          .catch((err) => setError(String(err)));
        setSelected(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected, doSave, doOpen]);

  // preview: topmost clip (V2 over V1) under the playhead
  const underPlayhead = useMemo(() => {
    for (const track of TRACKS) {
      const clip = clips.find(
        (c) => c.track === track && playhead >= c.start && playhead < c.start + c.len
      );
      if (!clip) continue;
      const m = media[clip.media];
      if (!m || m.thumbs.length === 0) continue;
      const srcT = playhead - clip.start + clip.src_in;
      return { media: m, srcT };
    }
    return null;
  }, [clips, media, playhead]);

  const proxySrc = useMemo(() => {
    if (!underPlayhead) return null;
    const { media: m, srcT } = underPlayhead;
    const idx = Math.min(m.thumbs.length - 1, Math.max(0, Math.floor(srcT * m.scrub_fps)));
    return m.thumbs[idx];
  }, [underPlayhead]);

  // when the playhead settles, snap the preview from the scrub proxy to a
  // full-quality frame from the decode engine
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
        /* engine miss is non-fatal — proxy frame stays up */
      }
    }, 250);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hqKey, drag, playing]);

  const previewSrc = hq && hq.key === hqKey ? hq.src : proxySrc;

  // ── transcript ──────────────────────────────────────────────────────
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

  // transcript shown: selected clip's media, else clip under playhead,
  // else the first transcribed media
  const transcriptMedia = useMemo(() => {
    const selClip = selected ? project.clips.find((c) => c.id === selected) : null;
    if (selClip && transcripts[selClip.media]) return selClip.media;
    if (underPlayhead && transcripts[underPlayhead.media.id]) return underPlayhead.media.id;
    return Object.keys(transcripts)[0] ?? null;
  }, [selected, project, transcripts, underPlayhead]);

  const onWordClick = useCallback(
    (mediaId: string, idx: number, words: Word[], shift: boolean) => {
      setWordSel((sel) =>
        shift && sel && sel.media === mediaId ? { ...sel, b: idx } : { media: mediaId, a: idx, b: idx }
      );
      const t = srcToTimeline(mediaId, (words[idx].start + words[idx].end) / 2);
      if (t != null) setPlayhead(t);
    },
    [srcToTimeline]
  );

  const cutWords = useCallback(async () => {
    if (!wordSel) return;
    const words = transcripts[wordSel.media];
    if (!words) return;
    const [i0, i1] = [Math.min(wordSel.a, wordSel.b), Math.max(wordSel.a, wordSel.b)];
    const from = words[i0].start;
    const to = words[i1].end;
    const mid = (from + to) / 2;
    const clip = project.clips.find(
      (c) =>
        c.media === wordSel.media &&
        c.track === "V1" &&
        mid >= c.src_in &&
        mid < c.src_in + c.len
    );
    if (!clip) {
      setError("No clip on V1 covers those words");
      return;
    }
    try {
      setProject(await razorOut(clip.id, from, to));
      setWordSel(null);
    } catch (e) {
      setError(String(e));
    }
  }, [wordSel, transcripts, project]);

  // suggested cuts: filler words + long silences, from word timestamps —
  // only ranges still present on the timeline
  const smartCuts = useMemo(() => {
    if (!transcriptMedia) return null;
    const words = transcripts[transcriptMedia];
    if (!words || words.length === 0) return null;
    const alive = (r: [number, number]) =>
      srcToTimeline(transcriptMedia, (r[0] + r[1]) / 2) !== null;
    const fillers: [number, number][] = words
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
  }, [transcriptMedia, transcripts, srcToTimeline]);

  const doCutRanges = useCallback(
    async (ranges: [number, number][]) => {
      if (!transcriptMedia || ranges.length === 0) return;
      try {
        setProject(await cutRanges(transcriptMedia, ranges));
        setWordSel(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [transcriptMedia]
  );

  // live caption: the word under the playhead
  const caption = useMemo(() => {
    if (!underPlayhead) return null;
    const words = transcripts[underPlayhead.media.id];
    if (!words) return null;
    const srcT = underPlayhead.srcT;
    return words.find((w) => srcT >= w.start && srcT < w.end)?.text ?? null;
  }, [underPlayhead, transcripts]);

  const doImport = useCallback(async () => {
    setError(null);
    const path = await pickVideo();
    if (!path) return;
    setBusy(`Importing ${path.split(/[\\/]/).pop()} — building scrub proxy…`);
    try {
      const res = await importMedia(path);
      setMedia((m) => ({ ...m, [res.media.id]: res.media }));
      setProject(res.project);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, []);

  // ── scrubbing (ruler + lanes background) ────────────────────────────
  const scrubTo = useCallback(
    (clientX: number) => {
      const lanes = lanesRef.current;
      if (!lanes) return;
      const x = clientX - lanes.getBoundingClientRect().left;
      setPlayhead(Math.max(0, x / pps));
    },
    [pps]
  );

  // ctrl+wheel zoom (non-passive listener so preventDefault works)
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      setPps((z) =>
        Math.min(PPS_MAX, Math.max(PPS_MIN, z * (e.deltaY < 0 ? 1.25 : 0.8)))
      );
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

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

  // ── clip dragging: move, or trim when grabbing an edge ─────────────
  const onClipPointerDown = useCallback(
    (e: React.PointerEvent, clip: Clip) => {
      e.stopPropagation();
      capture(e);
      setSelected(clip.id);
      setPlaying(false);
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
    [pps]
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
        const lane = Math.min(TRACKS.length - 1, Math.max(0, Math.floor(y / TRACK_H)));
        const start = Math.max(0, t - drag.grabOffsetS);
        setDrag({ ...drag, track: TRACKS[lane], start });
        setPlayhead(start);
        return;
      }

      const { orig } = drag;
      const clip = project.clips.find((c) => c.id === drag.clipId);
      const srcLen = clip ? media[clip.media]?.duration_s ?? Infinity : Infinity;
      if (drag.kind === "trim-l") {
        // dragging the in-point: start/src_in shift together, len absorbs
        const d = Math.max(-orig.srcIn, Math.min(t - orig.start, orig.len - MIN_LEN_S));
        const start = orig.start + d;
        setDrag({ ...drag, start, len: orig.len - d, srcIn: orig.srcIn + d });
        setPlayhead(start);
      } else {
        // dragging the out-point: only len changes, capped by source media
        const len = Math.max(MIN_LEN_S, Math.min(t - orig.start, srcLen - orig.srcIn));
        setDrag({ ...drag, len });
        setPlayhead(orig.start + len);
      }
    },
    [drag, project, media, pps]
  );

  const onClipPointerUp = useCallback(async () => {
    if (!drag) return;
    setDrag(null);
    const r2 = (v: number) => Math.round(v * 100) / 100;
    try {
      if (drag.kind === "move") {
        setProject(await moveClip(drag.clipId, drag.track, r2(drag.start)));
      } else {
        setProject(await trimClip(drag.clipId, r2(drag.start), r2(drag.len), r2(drag.srcIn)));
      }
    } catch (e) {
      setError(String(e));
    }
  }, [drag]);

  // ── render ──────────────────────────────────────────────────────────
  const mediaList = Object.values(media);
  const ticks = useMemo(
    () => Array.from({ length: Math.ceil(timelineEndS) + 1 }, (_, i) => i),
    [timelineEndS]
  );

  return (
    <div className="app">
      <header className="topbar">
        <span className="logo">⚔️ Cutlass</span>
        <span className="project-name">{project.name}</span>
        <button
          className="ghost-btn"
          title="Ctrl+Z"
          onClick={() => undoEdit().then(setProject).catch((e) => setError(String(e)))}
        >
          ↩
        </button>
        <button
          className="ghost-btn slim"
          title="Ctrl+Y"
          onClick={() => redoEdit().then(setProject).catch((e) => setError(String(e)))}
        >
          ↪
        </button>
        {room ? (
          <span className="badge live">🔗 {room}</span>
        ) : (
          <button className="ghost-btn slim" onClick={doCollab} disabled={!inTauri}>
            Collab
          </button>
        )}
        <button className="ghost-btn" onClick={doOpen} title="Ctrl+O">
          Open
        </button>
        <button className="ghost-btn" onClick={doSave} title="Ctrl+S">
          Save
        </button>
        <button
          className="ghost-btn"
          onClick={doExport}
          disabled={!inTauri || exporting !== null || clips.length === 0}
        >
          {exporting !== null ? `Exporting ${Math.round(exporting * 100)}%` : "Export"}
        </button>
        <button
          className="play-btn"
          onClick={() => setPlaying((p) => !p)}
          disabled={clips.length === 0}
          title="Space"
        >
          {playing ? "❚❚" : "▶"}
        </button>
        <button
          className="ghost-btn slim"
          title="Zoom out (Ctrl+wheel)"
          onClick={() => setPps((z) => Math.max(PPS_MIN, z * 0.8))}
        >
          −
        </button>
        <button
          className="ghost-btn slim"
          title="Zoom in (Ctrl+wheel)"
          onClick={() => setPps((z) => Math.min(PPS_MAX, z * 1.25))}
        >
          +
        </button>
        <button className="import-btn" onClick={doImport} disabled={busy !== null}>
          {busy ? "Importing…" : "Import media"}
        </button>
        {!inTauri && <span className="badge">browser mock</span>}
      </header>

      {busy && <div className="notice">{busy}</div>}
      {error && (
        <div className="notice error" onClick={() => setError(null)}>
          {error} (click to dismiss)
        </div>
      )}

      <main className="workspace">
        <aside className="bin">
          <h2>Media</h2>
          {mediaList.length === 0 && (
            <p className="hint">
              Import a video to begin.
              <br />
              Scrub proxies are generated on import; scrubbing never touches the source file.
            </p>
          )}
          {mediaList.map((m) => (
            <div className="bin-item" key={m.id}>
              <img src={m.thumbs[0]} alt="" />
              <div>
                <div className="bin-name">{m.name}</div>
                <div className="bin-meta">
                  {m.duration_s.toFixed(1)}s · {m.thumbs.length} proxy frames
                </div>
                {transcripts[m.id] ? (
                  <div className="bin-meta">📝 {transcripts[m.id].length} words</div>
                ) : (
                  <button
                    className="mini-btn"
                    disabled={transcribing !== null}
                    onClick={() => doTranscribe(m.id)}
                  >
                    {transcribing === m.id ? "Transcribing…" : "Transcribe"}
                  </button>
                )}
              </div>
            </div>
          ))}
        </aside>

        <section className="preview">
          {previewSrc ? (
            <img src={previewSrc} alt="preview" />
          ) : (
            <div className="preview-empty">no clip under playhead</div>
          )}
          {caption && <div className="caption">{caption}</div>}
          <div className="tc">{formatTC(playhead)}</div>
        </section>

        {transcriptMedia && transcripts[transcriptMedia] && (
          <aside className="transcript">
            <h2>Transcript</h2>
            <p className="hint">
              Click a word to seek · Shift-click to select a range · Cut removes it from the
              video
            </p>
            <div className="words">
              {transcripts[transcriptMedia].map((w, i) => {
                const sel =
                  wordSel &&
                  wordSel.media === transcriptMedia &&
                  i >= Math.min(wordSel.a, wordSel.b) &&
                  i <= Math.max(wordSel.a, wordSel.b);
                const cut = srcToTimeline(transcriptMedia, (w.start + w.end) / 2) === null;
                const filler = FILLERS.has(cleanWord(w.text));
                return (
                  <span
                    key={i}
                    className={`word${sel ? " sel" : ""}${cut ? " cut" : ""}${filler ? " filler" : ""}`}
                    onClick={(e) =>
                      onWordClick(transcriptMedia, i, transcripts[transcriptMedia], e.shiftKey)
                    }
                  >
                    {w.text}
                  </span>
                );
              })}
            </div>
            {wordSel && wordSel.media === transcriptMedia && (
              <button className="cut-btn" onClick={cutWords}>
                ✂ Cut {Math.abs(wordSel.b - wordSel.a) + 1} word
                {wordSel.a === wordSel.b ? "" : "s"} from video
              </button>
            )}
            {smartCuts && (smartCuts.fillers.length > 0 || smartCuts.silences.length > 0) && (
              <div className="smart-cuts">
                {smartCuts.fillers.length > 0 && (
                  <button className="smart-btn" onClick={() => doCutRanges(smartCuts.fillers)}>
                    ✂ {smartCuts.fillers.length} filler word
                    {smartCuts.fillers.length === 1 ? "" : "s"}
                  </button>
                )}
                {smartCuts.silences.length > 0 && (
                  <button className="smart-btn" onClick={() => doCutRanges(smartCuts.silences)}>
                    ✂ {smartCuts.silences.length} silence
                    {smartCuts.silences.length === 1 ? "" : "s"} (≥{SILENCE_GAP_S}s)
                  </button>
                )}
              </div>
            )}
          </aside>
        )}
      </main>

      <section className="timeline">
        <div className="timeline-scroll" ref={scrollRef}>
          <div className="timeline-inner" style={{ width: timelineEndS * pps }}>
            <div
              className="ruler"
              onPointerDown={onRulerPointerDown}
              onPointerMove={onRulerPointerMove}
            >
              {ticks.map((s) => (
                <div className="tick" key={s} style={{ left: s * pps }}>
                  {s % 5 === 0 && <span>{formatTC(s)}</span>}
                </div>
              ))}
            </div>

            <div
              className="lanes"
              ref={lanesRef}
              onPointerDown={() => setSelected(null)}
            >
              {TRACKS.map((track) => (
                <div className="lane" key={track} style={{ height: TRACK_H }}>
                  <span className="lane-label">{track}</span>
                  {clips
                    .filter((c) => c.track === track)
                    .map((clip) => (
                      <ClipView
                        key={clip.id}
                        clip={clip}
                        media={media[clip.media]}
                        pps={pps}
                        dragging={drag?.clipId === clip.id}
                        selected={selected === clip.id}
                        onPointerDown={onClipPointerDown}
                        onPointerMove={onClipPointerMove}
                        onPointerUp={onClipPointerUp}
                      />
                    ))}
                </div>
              ))}
              <div className="playhead" style={{ left: playhead * pps }} />
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}

function ClipView({
  clip,
  media,
  pps,
  dragging,
  selected,
  onPointerDown,
  onPointerMove,
  onPointerUp,
}: {
  clip: Clip;
  media?: MediaItem;
  pps: number;
  dragging: boolean;
  selected: boolean;
  onPointerDown: (e: React.PointerEvent, clip: Clip) => void;
  onPointerMove: (e: React.PointerEvent) => void;
  onPointerUp: () => void;
}) {
  const w = clip.len * pps;
  const filmstrip = useMemo(() => {
    if (!media) return [];
    const cellW = 56;
    const n = Math.max(1, Math.floor(w / cellW));
    return Array.from({ length: n }, (_, i) => {
      const srcT = clip.src_in + (i / n) * clip.len;
      const idx = Math.min(media.thumbs.length - 1, Math.floor(srcT * media.scrub_fps));
      return media.thumbs[idx];
    });
  }, [media, w, clip.src_in, clip.len]);

  return (
    <div
      className={`clip${dragging ? " dragging" : ""}${selected ? " selected" : ""}`}
      style={{ left: clip.start * pps, width: w }}
      onPointerDown={(e) => onPointerDown(e, clip)}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <div className="clip-strip">
        {filmstrip.map((src, i) => (
          <img key={i} src={src} alt="" draggable={false} />
        ))}
      </div>
      <span className="clip-name">{clip.name}</span>
      <div className="trim-handle trim-l" />
      <div className="trim-handle trim-r" />
    </div>
  );
}

function formatTC(t: number): string {
  const m = Math.floor(t / 60);
  const s = Math.floor(t % 60);
  const f = Math.floor((t % 1) * 30);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(f).padStart(2, "0")}`;
}
