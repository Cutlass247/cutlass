import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Clip,
  MediaItem,
  ProjectSnapshot,
  exactFrame,
  getProject,
  importMedia,
  inTauri,
  moveClip,
  pickVideo,
  removeClip,
  trimClip,
} from "./ipc";

const PPS = 48; // timeline pixels per second

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
  const [drag, setDrag] = useState<DragState | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const lanesRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    getProject().then(setProject).catch((e) => setError(String(e)));
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

  // ── playback: interval clock over the proxy frames ─────────────────
  // interval + performance.now deltas rather than rAF: rAF stops dead in
  // hidden/minimized windows. Audio will own this clock eventually.
  useEffect(() => {
    if (!playing) return;
    let last = performance.now();
    const timer = setInterval(() => {
      const now = performance.now();
      const dt = (now - last) / 1000;
      last = now;
      setPlayhead((t) => {
        const nt = t + dt;
        if (nt >= contentEndS) {
          setPlaying(false);
          return contentEndS;
        }
        return nt;
      });
    }, 33);
    return () => clearInterval(timer);
  }, [playing, contentEndS]);

  // ── keyboard: space = play/pause, delete = lift, shift+delete = ripple
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.code === "Space") {
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
  }, [selected]);

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
  const scrubTo = useCallback((clientX: number) => {
    const lanes = lanesRef.current;
    if (!lanes) return;
    const x = clientX - lanes.getBoundingClientRect().left;
    setPlayhead(Math.max(0, x / PPS));
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
      const localPx = x - clip.start * PPS;
      const orig = { start: clip.start, len: clip.len, srcIn: clip.src_in };
      if (localPx <= TRIM_ZONE_PX) {
        setDrag({ kind: "trim-l", clipId: clip.id, ...orig, orig });
      } else if (localPx >= clip.len * PPS - TRIM_ZONE_PX) {
        setDrag({ kind: "trim-r", clipId: clip.id, ...orig, orig });
      } else {
        setDrag({
          kind: "move",
          clipId: clip.id,
          track: clip.track,
          start: clip.start,
          grabOffsetS: x / PPS - clip.start,
        });
      }
    },
    []
  );

  const onClipPointerMove = useCallback(
    (e: React.PointerEvent) => {
      if (!drag || !(e.buttons & 1)) return;
      const lanes = lanesRef.current!;
      const rect = lanes.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const t = x / PPS;

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
    [drag, project, media]
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
          className="play-btn"
          onClick={() => setPlaying((p) => !p)}
          disabled={clips.length === 0}
          title="Space"
        >
          {playing ? "❚❚" : "▶"}
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
          <div className="tc">{formatTC(playhead)}</div>
        </section>
      </main>

      <section className="timeline">
        <div className="timeline-scroll">
          <div className="timeline-inner" style={{ width: timelineEndS * PPS }}>
            <div
              className="ruler"
              onPointerDown={onRulerPointerDown}
              onPointerMove={onRulerPointerMove}
            >
              {ticks.map((s) => (
                <div className="tick" key={s} style={{ left: s * PPS }}>
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
                        dragging={drag?.clipId === clip.id}
                        selected={selected === clip.id}
                        onPointerDown={onClipPointerDown}
                        onPointerMove={onClipPointerMove}
                        onPointerUp={onClipPointerUp}
                      />
                    ))}
                </div>
              ))}
              <div className="playhead" style={{ left: playhead * PPS }} />
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
  dragging,
  selected,
  onPointerDown,
  onPointerMove,
  onPointerUp,
}: {
  clip: Clip;
  media?: MediaItem;
  dragging: boolean;
  selected: boolean;
  onPointerDown: (e: React.PointerEvent, clip: Clip) => void;
  onPointerMove: (e: React.PointerEvent) => void;
  onPointerUp: () => void;
}) {
  const w = clip.len * PPS;
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
      style={{ left: clip.start * PPS, width: w }}
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
