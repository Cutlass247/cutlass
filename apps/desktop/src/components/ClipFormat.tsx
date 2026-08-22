import { Segmented } from "./ui";

/// Monthly AI allowance (mirrors ipc.AiUsage). remaining_minutes = -1 ⇒ unlimited.
export type AiUsageInfo = {
  used_minutes: number;
  remaining_minutes: number;
  unlimited: boolean;
};

/// The output shape a Create clip is made for.
export type ClipFormatDef = { id: string; label: string; sub: string; w: number; h: number };
export const CLIP_FORMATS: ClipFormatDef[] = [
  { id: "vertical", label: "Vertical", sub: "Shorts · TikTok · Reels", w: 1080, h: 1920 },
  { id: "square", label: "Square", sub: "Feed posts", w: 1080, h: 1080 },
  { id: "wide", label: "Wide", sub: "YouTube · landscape", w: 1920, h: 1080 },
];

/// A candidate short: a source-time range with a preview label, and (for
/// AI-free highlight picks) the signals that made it stand out.
export type ShortSeg = { start: number; end: number; label: string; reasons?: string[] };

const clock = (s: number) => {
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return `${m}:${String(r).padStart(2, "0")}`;
};

/// The Create tab's clip maker: pick a platform shape, choose how the footage
/// fills it, let the AI find the best moments (each becomes a captioned clip),
/// and export. Everything for one short, in one panel.
export function ClipFormat(p: {
  format: ClipFormatDef;
  onFormat: (f: ClipFormatDef) => void;
  reframe: "fill" | "blur";
  onReframe: (r: "fill" | "blur") => void;
  reframeX: number;
  onReframeX: (x: number) => void;
  hasClip: boolean;
  // "Find best moments" transcribes (progress %) then hands the transcript to
  // the AI (aiFinding) — one button, two phases.
  transcribing: boolean;
  transcribePct: number;
  aiFinding: boolean;
  // how the transcription runs: "fast" = cloud GPU, "private" = on-device
  transcribeMode: "fast" | "private";
  onTranscribeMode: (m: "fast" | "private") => void;
  // captions are opt-in: only burned onto a clip when this is on
  captionsOn: boolean;
  onToggleCaptions: (on: boolean) => void;
  // monthly AI allowance readout (null = unknown/hidden)
  usage: AiUsageInfo | null;
  onFindHighlights: () => void;
  exporting: boolean;
  onExport: () => void;
  // manual fallback: chop one long clip into even short-ready segments
  canSplit: boolean;
  shorts: ShortSeg[];
  activeShort: number | null;
  onSplit: () => void;
  onPickShort: (i: number) => void;
}) {
  const finding = p.transcribing || p.aiFinding;
  const pct = Math.round(p.transcribePct);
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

      {/* speed vs privacy: cloud GPU (fast) or on-device (nothing leaves) */}
      <div className="cf-row">
        <span className="cf-lbl">Transcribe</span>
        <Segmented
          options={[
            { value: "fast", label: "Fast" },
            { value: "private", label: "Private" },
          ]}
          value={p.transcribeMode}
          onChange={(v) => p.onTranscribeMode(v as "fast" | "private")}
        />
      </div>
      <div className="cf-split-hint" style={{ marginTop: -2 }}>
        {p.transcribeMode === "fast"
          ? "⚡ Cloud GPU — seconds, even for long videos. Audio (not your video) is sent to transcribe, then discarded."
          : "🔒 On-device — nothing leaves your machine. Slower on long videos."}
      </div>

      {/* captions are opt-in — never burned on automatically */}
      <div className="cf-row">
        <span className="cf-lbl">Captions</span>
        <Segmented
          options={[
            { value: "off", label: "Off" },
            { value: "on", label: "On" },
          ]}
          value={p.captionsOn ? "on" : "off"}
          onChange={(v) => p.onToggleCaptions(v === "on")}
        />
      </div>
      <div className="cf-split-hint" style={{ marginTop: -2 }}>
        {p.captionsOn
          ? "On — the loaded clip gets captions from the transcript."
          : "Off — clips load without captions. Flip on any time to add them."}
      </div>

      {/* THE headline action: AI reads the whole clip and pulls the best
          moments — each loads as a short ready to export. */}
      <button
        className={`cf-highlights-btn${finding ? " loading" : ""}`}
        disabled={finding || !p.hasClip}
        onClick={p.onFindHighlights}
        title="Transcribes the clip, then the AI finds the funniest, most exciting and pivotal standout moments"
      >
        {p.transcribing && (
          <span className="cf-captions-fill" style={{ width: `${Math.max(3, p.transcribePct)}%` }} />
        )}
        {p.aiFinding && <span className="cf-ai-scan" />}
        <span className="cf-captions-label">
          {p.transcribing
            ? `Reading the video… ${pct}%`
            : p.aiFinding
            ? "✨ Finding the best moments…"
            : "✨ Find the best moments"}
        </span>
      </button>
      {p.usage &&
        (p.usage.unlimited ? (
          <div className="cf-usage">
            {Math.round(p.usage.used_minutes)} min of AI used this month · unlimited during beta
          </div>
        ) : (
          <div className={`cf-usage${p.usage.remaining_minutes <= 15 ? " low" : ""}`}>
            {Math.max(0, Math.round(p.usage.remaining_minutes))} min of AI left this month
          </div>
        ))}
      {!finding && p.shorts.length === 0 && (
        <div className="cf-split-hint">
          The AI scans your whole clip for standout moments and turns each into a
          short{p.captionsOn ? " — with captions baked in." : "."}
        </div>
      )}

      {p.shorts.length > 0 && (
        <div className="cf-shorts-wrap">
          <div className="cf-split-hint">
            Pick a moment to load it{p.captionsOn ? " (with captions)" : ""} — then Export it in the
            shape above.
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
                  {s.reasons && s.reasons.length > 0 && (
                    <span className="cf-short-reasons">{s.reasons.join("  ·  ")}</span>
                  )}
                  <span className="cf-short-time">
                    {clock(s.start)}–{clock(s.end)} · {Math.round(s.end - s.start)}s
                  </span>
                </span>
              </button>
            ))}
          </div>
        </div>
      )}

      <button className="cf-export" disabled={!p.hasClip || p.exporting} onClick={p.onExport}>
        {p.exporting ? "Exporting…" : `Export ${p.format.label} clip`}
      </button>

      {p.canSplit && (
        <button className="cf-split-btn" disabled={finding} onClick={p.onSplit}>
          ✂ Or split into even shorts
        </button>
      )}
    </div>
  );
}
