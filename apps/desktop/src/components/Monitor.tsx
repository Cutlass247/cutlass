import { ReactNode, useEffect, useRef, useState } from "react";
import { formatTC, IconButton } from "./ui";

/// A monitor layer with the green chroma-keyed to transparent on a canvas,
/// so lower layers show through. Approximate preview; export is exact.
function KeyedLayer(p: { src: string; sim: number; style?: React.CSSProperties }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const img = new Image();
    img.onload = () => {
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      ctx.drawImage(img, 0, 0);
      const data = ctx.getImageData(0, 0, canvas.width, canvas.height);
      const d = data.data;
      const t = 1.1 + (0.8 - Math.min(0.8, p.sim)); // higher sim = looser key
      for (let i = 0; i < d.length; i += 4) {
        const r = d[i];
        const g = d[i + 1];
        const b = d[i + 2];
        if (g > 70 && g > r * t && g > b * t) d[i + 3] = 0;
      }
      ctx.putImageData(data, 0, 0);
    };
    img.src = p.src;
  }, [p.src, p.sim]);
  return <canvas ref={ref} style={p.style} />;
}

export type Resolution = { label: string; w: number; h: number };
export const RESOLUTIONS: Resolution[] = [
  { label: "720p", w: 1280, h: 720 },
  { label: "1080p", w: 1920, h: 1080 },
  { label: "4K", w: 3840, h: 2160 },
];

export function Monitor(p: {
  layers: {
    key: string;
    src: string;
    style?: React.CSSProperties;
    chroma?: boolean;
    chromaSim?: number;
  }[];
  vignette?: number;
  grain?: number;
  titleOverlay?: ReactNode;
  caption: string | null;
  playhead: number;
  playing: boolean;
  canPlay: boolean;
  resolution: Resolution;
  onResolution: (r: Resolution) => void;
  onTogglePlay: () => void;
  onSeek: (t: number) => void;
  contentEnd: number;
}) {
  const [safe, setSafe] = useState(false);
  const frameRef = useRef<HTMLDivElement>(null);

  return (
    <section className="monitor">
      <div className="monitor-frame" ref={frameRef}>
        {p.layers.length ? (
          <div className="monitor-layers">
            {p.layers.map((l) =>
              l.chroma ? (
                <KeyedLayer key={l.key} src={l.src} sim={l.chromaSim ?? 0.3} style={l.style} />
              ) : (
                <img key={l.key} src={l.src} alt="" draggable={false} style={l.style} />
              )
            )}
          </div>
        ) : (
          <div className="monitor-empty">No clip under the playhead</div>
        )}
        {p.grain && p.grain > 0.01 ? (
          <div className="grain-overlay" style={{ opacity: Math.min(0.5, p.grain * 0.5) }} />
        ) : null}
        {p.vignette && p.vignette > 0.01 ? (
          <div className="vignette-overlay" style={{ opacity: Math.min(1, p.vignette) }} />
        ) : null}
        {safe && (
          <>
            <div className="safe action" />
            <div className="safe title" />
          </>
        )}
        {p.titleOverlay}
        {p.caption && <div className="caption">{p.caption}</div>}
      </div>
      <div className="transport">
        <IconButton label="Jump to start" hint="Home" onClick={() => p.onSeek(0)}>
          ⏮
        </IconButton>
        <IconButton
          label={p.playing ? "Pause" : "Play"}
          hint="Space"
          onClick={p.onTogglePlay}
          disabled={!p.canPlay}
          active={p.playing}
        >
          {p.playing ? "❚❚" : "▶"}
        </IconButton>
        <IconButton label="Jump to end" hint="End" onClick={() => p.onSeek(p.contentEnd)}>
          ⏭
        </IconButton>
        <span className="tc" title="Playhead timecode">
          {formatTC(p.playhead)}
        </span>
        <span className="spacer" />
        <select
          className="res-select"
          title="Export resolution"
          value={p.resolution.label}
          onChange={(e) =>
            p.onResolution(RESOLUTIONS.find((r) => r.label === e.target.value) ?? RESOLUTIONS[1])
          }
        >
          {RESOLUTIONS.map((r) => (
            <option key={r.label}>{r.label}</option>
          ))}
        </select>
        <IconButton label="Safe margins" active={safe} onClick={() => setSafe((s) => !s)}>
          ⛶
        </IconButton>
        <IconButton
          label="Fullscreen"
          onClick={() => frameRef.current?.requestFullscreen?.().catch(() => {})}
        >
          ⤢
        </IconButton>
      </div>
    </section>
  );
}
