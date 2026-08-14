import { Segmented } from "./ui";

/// The output shape a Create clip is made for.
export type ClipFormatDef = { id: string; label: string; sub: string; w: number; h: number };
export const CLIP_FORMATS: ClipFormatDef[] = [
  { id: "vertical", label: "Vertical", sub: "Shorts · TikTok · Reels", w: 1080, h: 1920 },
  { id: "square", label: "Square", sub: "Feed posts", w: 1080, h: 1080 },
  { id: "wide", label: "Wide", sub: "YouTube · landscape", w: 1920, h: 1080 },
];

/// A candidate short: a source-time range with a preview label.
export type ShortSeg = { start: number; end: number; label: string };

const clock = (s: number) => {
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return `${m}:${String(r).padStart(2, "0")}`;
};

/// The Create tab's clip maker: pick a platform shape, choose how the footage
/// fills it, add captions, and export. Everything for one short, in one panel.
export function ClipFormat(p: {
  format: ClipFormatDef;
  onFormat: (f: ClipFormatDef) => void;
  reframe: "fill" | "blur";
  onReframe: (r: "fill" | "blur") => void;
  reframeX: number;
  onReframeX: (x: number) => void;
  hasClip: boolean;
  captionsReady: boolean;
  transcribing: boolean;
  onAddCaptions: () => void;
  exporting: boolean;
  onExport: () => void;
  // auto-split: turn one long clip into several short-ready segments
  canSplit: boolean;
  shorts: ShortSeg[];
  activeShort: number | null;
  onSplit: () => void;
  onPickShort: (i: number) => void;
}) {
  return (
    <div className="clip-format">
      <div className="cf-title">Make a clip for</div>
      <div className="cf-formats">
        {CLIP_FORMATS.map((f) => (
          <button
            key={f.id}
            className={`cf-fmt${p.format.id === f.id ? " on" : ""}`}
            onClick={() => p.onFormat(f)}
            title={`${f.w}×${f.h}`}
          >
            <span className="cf-shape-wrap">
              <span className="cf-shape" style={{ aspectRatio: `${f.w} / ${f.h}` }} />
            </span>
            <span className="cf-fmt-label">{f.label}</span>
            <span className="cf-fmt-sub">{f.sub}</span>
          </button>
        ))}
      </div>

      <div className="cf-row">
        <span className="cf-lbl">Fit footage</span>
        <Segmented
          options={[
            { value: "fill", label: "Fill" },
            { value: "blur", label: "Blur bg" },
          ]}
          value={p.reframe}
          onChange={(v) => p.onReframe(v as "fill" | "blur")}
        />
      </div>

      {p.reframe === "fill" && (
        <div className="cf-row">
          <span className="cf-lbl">Pan</span>
          <input
            className="cf-pan"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={p.reframeX}
            onChange={(e) => p.onReframeX(Number(e.target.value))}
            title="Which part of the footage stays in frame"
          />
        </div>
      )}

      <button
        className="cf-captions"
        disabled={p.transcribing || !p.hasClip}
        onClick={p.onAddCaptions}
        title="Transcribe on-device and burn captions"
      >
        {p.transcribing ? "Adding captions…" : p.captionsReady ? "✓ Captions added" : "✨ Add captions"}
      </button>

      <button className="cf-export" disabled={!p.hasClip || p.exporting} onClick={p.onExport}>
        {p.exporting ? "Exporting…" : `Export ${p.format.label} clip`}
      </button>

      {p.canSplit && (
        <div className="cf-split">
          <button className="cf-split-btn" onClick={p.onSplit}>
            ✂ Split into shorts
          </button>
          {p.shorts.length > 0 && (
            <>
              <div className="cf-split-hint">
                Pick a segment to load it — then Export it in the shape above.
              </div>
              <div className="cf-shorts">
                {p.shorts.map((s, i) => (
                  <button
                    key={i}
                    className={`cf-short${p.activeShort === i ? " on" : ""}`}
                    onClick={() => p.onPickShort(i)}
                    title={s.label}
                  >
                    <span className="cf-short-n">{i + 1}</span>
                    <span className="cf-short-body">
                      <span className="cf-short-label">{s.label}</span>
                      <span className="cf-short-time">
                        {clock(s.start)}–{clock(s.end)} · {Math.round(s.end - s.start)}s
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
