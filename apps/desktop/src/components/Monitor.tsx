import { useRef, useState } from "react";
import { formatTC, IconButton } from "./ui";

export type Resolution = { label: string; w: number; h: number };
export const RESOLUTIONS: Resolution[] = [
  { label: "720p", w: 1280, h: 720 },
  { label: "1080p", w: 1920, h: 1080 },
  { label: "4K", w: 3840, h: 2160 },
];

export function Monitor(p: {
  src: string | null;
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
        {p.src ? (
          <img src={p.src} alt="program monitor" draggable={false} />
        ) : (
          <div className="monitor-empty">No clip under the playhead</div>
        )}
        {safe && (
          <>
            <div className="safe action" />
            <div className="safe title" />
          </>
        )}
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
