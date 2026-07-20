import { RefObject, useMemo } from "react";
import { Clip, MediaItem, Presence } from "../ipc";
import { formatTC, mediaHue, Switch } from "./ui";

export const TRACK_H = 64;
export const PPS_MIN = 12;
export const PPS_MAX = 160;

export interface TrackCtl {
  lock: boolean;
  mute: boolean;
  hide: boolean;
}

export function Timeline(p: {
  tracks: string[];
  clips: Clip[];
  media: Record<string, MediaItem>;
  pps: number;
  onPps: (v: number) => void;
  playhead: number;
  peers: Presence[];
  selected: string | null;
  dragClipId: string | null;
  timelineEndS: number;
  snap: boolean;
  onSnap: (v: boolean) => void;
  markers: number[];
  trackCtl: Record<string, TrackCtl>;
  onTrackCtl: (track: string, patch: Partial<TrackCtl>) => void;
  height: number;
  lanesRef: RefObject<HTMLDivElement>;
  scrollRef: RefObject<HTMLDivElement>;
  onRulerPointerDown: (e: React.PointerEvent) => void;
  onRulerPointerMove: (e: React.PointerEvent) => void;
  onLanesPointerDown: () => void;
  onClipPointerDown: (e: React.PointerEvent, clip: Clip) => void;
  onClipPointerMove: (e: React.PointerEvent) => void;
  onClipPointerUp: () => void;
  onSeek: (t: number) => void;
  showTrackHeads: boolean;
}) {
  const ticks = useMemo(
    () => Array.from({ length: Math.ceil(p.timelineEndS) + 1 }, (_, i) => i),
    [p.timelineEndS]
  );
  const labelEvery = p.pps < 24 ? 10 : p.pps < 60 ? 5 : 1;

  return (
    <section className="timeline" style={{ height: p.height }}>
      <div className="tl-toolbar">
        <Switch label="Snap" checked={p.snap} onChange={p.onSnap} />
        <span className="tl-hint">
          M marker · drag edges to trim · Shift+Del ripple
        </span>
        <span className="spacer" />
        <span className="tl-zoom-label">🔍</span>
        <input
          className="zoom-slider"
          type="range"
          min={PPS_MIN}
          max={PPS_MAX}
          value={p.pps}
          onChange={(e) => p.onPps(Number(e.target.value))}
          title="Timeline zoom (Ctrl+wheel)"
        />
      </div>
      <div className="tl-body">
        {p.showTrackHeads && (
          <div className="track-heads">
            <div className="track-head ruler-spacer" />
            {p.tracks.map((t) => {
              const ctl = p.trackCtl[t] ?? { lock: false, mute: false, hide: false };
              return (
                <div className="track-head" key={t} style={{ height: TRACK_H }}>
                  <span className="track-name">{t}</span>
                  <button
                    className={`track-btn${ctl.lock ? " on" : ""}`}
                    title={ctl.lock ? "Unlock track" : "Lock track"}
                    onClick={() => p.onTrackCtl(t, { lock: !ctl.lock })}
                  >
                    {ctl.lock ? "🔒" : "🔓"}
                  </button>
                  <button
                    className={`track-btn${ctl.mute ? " on" : ""}`}
                    title={ctl.mute ? "Unmute audio" : "Mute audio"}
                    onClick={() => p.onTrackCtl(t, { mute: !ctl.mute })}
                  >
                    {ctl.mute ? "🔇" : "🔊"}
                  </button>
                  <button
                    className={`track-btn${ctl.hide ? " on" : ""}`}
                    title={ctl.hide ? "Show in program" : "Hide from program"}
                    onClick={() => p.onTrackCtl(t, { hide: !ctl.hide })}
                  >
                    {ctl.hide ? "🚫" : "👁"}
                  </button>
                </div>
              );
            })}
          </div>
        )}
        <div className="timeline-scroll" ref={p.scrollRef}>
          <div className="timeline-inner" style={{ width: p.timelineEndS * p.pps }}>
            <div
              className="ruler"
              onPointerDown={p.onRulerPointerDown}
              onPointerMove={p.onRulerPointerMove}
            >
              {ticks.map((s) => (
                <div className="tick" key={s} style={{ left: s * p.pps }}>
                  {s % labelEvery === 0 && <span>{formatTC(s)}</span>}
                </div>
              ))}
              {p.markers.map((m, i) => (
                <div
                  className="marker"
                  key={i}
                  style={{ left: m * p.pps }}
                  title={`Marker ${formatTC(m)} — click to jump`}
                  onPointerDown={(e) => {
                    e.stopPropagation();
                    p.onSeek(m);
                  }}
                />
              ))}
            </div>
            <div className="lanes" ref={p.lanesRef} onPointerDown={p.onLanesPointerDown}>
              {p.tracks.map((track) => {
                const ctl = p.trackCtl[track] ?? { lock: false, mute: false, hide: false };
                return (
                  <div
                    className={`lane${ctl.hide ? " hidden-track" : ""}${ctl.lock ? " locked" : ""}`}
                    key={track}
                    style={{ height: TRACK_H }}
                  >
                    {p.clips
                      .filter((c) => c.track === track)
                      .map((clip) => (
                        <ClipView
                          key={clip.id}
                          clip={clip}
                          media={p.media[clip.media]}
                          pps={p.pps}
                          dragging={p.dragClipId === clip.id}
                          selected={p.selected === clip.id}
                          locked={ctl.lock}
                          onPointerDown={p.onClipPointerDown}
                          onPointerMove={p.onClipPointerMove}
                          onPointerUp={p.onClipPointerUp}
                        />
                      ))}
                  </div>
                );
              })}
              {p.peers.map((peer) => (
                <div
                  className="peer-playhead"
                  key={peer.id}
                  style={{ left: peer.playhead * p.pps, background: peer.color }}
                >
                  <span style={{ background: peer.color }}>{peer.name}</span>
                </div>
              ))}
              <div className="playhead" style={{ left: p.playhead * p.pps }} />
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function ClipView({
  clip,
  media,
  pps,
  dragging,
  selected,
  locked,
  onPointerDown,
  onPointerMove,
  onPointerUp,
}: {
  clip: Clip;
  media?: MediaItem;
  pps: number;
  dragging: boolean;
  selected: boolean;
  locked: boolean;
  onPointerDown: (e: React.PointerEvent, clip: Clip) => void;
  onPointerMove: (e: React.PointerEvent) => void;
  onPointerUp: () => void;
}) {
  const w = clip.len * pps;
  const isTitle = !!clip.text;
  const hue = isTitle ? 265 : mediaHue(clip.media);

  const filmstrip = useMemo(() => {
    if (!media || isTitle) return [];
    // Cap the thumbnail count so a long clip doesn't spawn thousands of
    // <img> nodes and freeze the timeline; they flex-stretch to fill.
    const cellW = 56;
    const n = Math.max(1, Math.min(48, Math.floor(w / cellW)));
    const speed = clip.fx?.speed ?? 1;
    return Array.from({ length: n }, (_, i) => {
      const srcT = clip.src_in + (i / n) * clip.len * speed;
      const idx = Math.min(media.thumbs.length - 1, Math.floor(srcT * media.scrub_fps));
      return media.thumbs[idx];
    });
  }, [media, w, clip.src_in, clip.len, clip.fx?.speed]);

  const wave = useMemo(() => {
    const wf = media?.waveform;
    if (!media || !wf || wf.length < 2) return null;
    const dur = media.duration_s || 1;
    const i0 = Math.max(0, Math.floor((clip.src_in / dur) * wf.length));
    const i1 = Math.min(wf.length, Math.ceil(((clip.src_in + clip.len) / dur) * wf.length));
    const slice = wf.slice(i0, i1);
    const n = Math.min(220, slice.length);
    if (n < 2) return null;
    const pts: string[] = [];
    for (let i = 0; i < n; i++) {
      const v = slice[Math.floor((i / n) * slice.length)] ?? 0;
      pts.push(`${i},${30 - v * 28}`);
    }
    return { d: `M0,30 L${pts.join(" L")} L${n - 1},30 Z`, n };
  }, [media, clip.src_in, clip.len]);

  return (
    <div
      className={`clip${dragging ? " dragging" : ""}${selected ? " selected" : ""}${locked ? " locked" : ""}${isTitle ? " title-clip" : ""}`}
      style={{
        left: clip.start * pps,
        width: w,
        borderColor: `hsl(${hue} 55% ${selected ? 62 : 40}%)`,
        ...(isTitle ? { background: `hsl(${hue} 45% 26%)` } : {}),
      }}
      onPointerDown={(e) => onPointerDown(e, clip)}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      {isTitle ? (
        <span className="title-clip-label">T {clip.text}</span>
      ) : (
        <div className="clip-strip">
          {filmstrip.map((src, i) => (
            <img key={i} src={src} alt="" draggable={false} />
          ))}
        </div>
      )}
      {wave && (
        <svg className="clip-wave" viewBox={`0 0 ${wave.n - 1} 30`} preserveAspectRatio="none">
          <path d={wave.d} />
        </svg>
      )}
      <span className="clip-name" style={{ background: `hsl(${hue} 55% 30% / 0.9)` }}>
        {clip.name}
      </span>
      {(clip.fx?.trans_dur ?? 0) > 0.05 && (
        <div
          className="trans-badge"
          title={`${clip.fx?.trans_dip ? "Dip to black" : "Cross dissolve"} ${(clip.fx?.trans_dur ?? 0).toFixed(1)}s`}
        >
          ⧓
        </div>
      )}
      {Math.abs((clip.fx?.speed ?? 1) - 1) > 0.01 && (
        <div className="speed-badge">{(clip.fx?.speed ?? 1).toFixed(2).replace(/\.?0+$/, "")}×</div>
      )}
      <div className="trim-handle trim-l" />
      <div className="trim-handle trim-r" />
    </div>
  );
}
