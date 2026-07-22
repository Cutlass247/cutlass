import { useEffect, useState } from "react";
import { Clip, FX_DEFAULTS, fxValue, fxValueAt, kfPoints, MediaItem, Word } from "../ipc";
import { formatTC, mediaHue, Segmented } from "./ui";

type Tab = "inspector" | "transcript";

interface FxParam {
  key: string;
  label: string;
  min: number;
  max: number;
  step: number;
  fmt?: (v: number) => string;
}

const FX_GROUPS: { title: string; params: FxParam[] }[] = [
  {
    title: "Color",
    params: [
      { key: "brightness", label: "Brightness", min: -1, max: 1, step: 0.02 },
      { key: "contrast", label: "Contrast", min: 0, max: 2, step: 0.02 },
      { key: "saturation", label: "Saturation", min: 0, max: 2, step: 0.02 },
    ],
  },
  {
    title: "Transform",
    params: [
      { key: "scale", label: "Scale", min: 0.1, max: 3, step: 0.02 },
      { key: "pos_x", label: "Position X", min: -0.5, max: 0.5, step: 0.01 },
      { key: "pos_y", label: "Position Y", min: -0.5, max: 0.5, step: 0.01 },
      { key: "rot", label: "Rotation", min: -180, max: 180, step: 1, fmt: (v) => `${v}°` },
    ],
  },
  {
    title: "Stylize",
    params: [
      { key: "hue", label: "Hue", min: -180, max: 180, step: 1, fmt: (v) => `${Math.round(v)}°` },
      { key: "blur", label: "Blur", min: 0, max: 20, step: 0.5 },
      { key: "vignette", label: "Vignette", min: 0, max: 1, step: 0.05 },
      { key: "flip_h", label: "Mirror H", min: 0, max: 1, step: 1, fmt: (v) => (v > 0.5 ? "on" : "off") },
      { key: "flip_v", label: "Flip V", min: 0, max: 1, step: 1, fmt: (v) => (v > 0.5 ? "on" : "off") },
    ],
  },
  {
    title: "Fade & audio",
    params: [
      { key: "fade_in", label: "Fade in", min: 0, max: 3, step: 0.1, fmt: (v) => `${v.toFixed(1)}s` },
      { key: "fade_out", label: "Fade out", min: 0, max: 3, step: 0.1, fmt: (v) => `${v.toFixed(1)}s` },
      { key: "volume", label: "Volume", min: 0, max: 2, step: 0.02 },
    ],
  },
];

/// Slider row with keyframe support. Constant mode: live preview on
/// drag, one committed edit on release, double-click label resets. ◆
/// writes a keyframe at the playhead — once keyframed the shown value
/// tracks the animation and edits update the keyframe there; ✕ clears.
function FxSlider({
  param,
  clip,
  clipTime,
  onPreview,
  onCommit,
  onSetKeyframe,
  onClearKeyframes,
}: {
  param: FxParam;
  clip: Clip;
  clipTime: number;
  onPreview: (key: string, v: number) => void;
  onCommit: (key: string, v: number) => void;
  onSetKeyframe: (key: string, t: number, v: number) => void;
  onClearKeyframes: (key: string) => void;
}) {
  const points = kfPoints(clip, param.key);
  const keyframed = points.length > 0;
  const value = keyframed ? fxValueAt(clip, param.key, clipTime) : fxValue(clip, param.key);
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const isDefault = !keyframed && Math.abs(draft - (FX_DEFAULTS[param.key] ?? 0)) < 1e-9;
  const fmt = param.fmt ?? ((v: number) => v.toFixed(2));
  const commit = (v: number) =>
    keyframed ? onSetKeyframe(param.key, clipTime, v) : onCommit(param.key, v);

  return (
    <div className={`fx-row${isDefault ? "" : " active"}${keyframed ? " kf" : ""}`}>
      <span
        className="fx-label"
        title="Double-click to reset"
        onDoubleClick={() => {
          if (keyframed) return;
          const d = FX_DEFAULTS[param.key] ?? 0;
          setDraft(d);
          onCommit(param.key, d);
        }}
      >
        {param.label}
      </span>
      <input
        type="range"
        min={param.min}
        max={param.max}
        step={param.step}
        value={draft}
        onChange={(e) => {
          const v = Number(e.target.value);
          setDraft(v);
          onPreview(param.key, v);
        }}
        onPointerUp={() => commit(draft)}
        onKeyUp={() => commit(draft)}
      />
      <span className="fx-val">{fmt(draft)}</span>
      <button
        className={`kf-btn${keyframed ? " on" : ""}`}
        title={
          keyframed
            ? `Keyframe at ${clipTime.toFixed(1)}s (${points.length} total)`
            : "Add keyframe at playhead"
        }
        onClick={() => onSetKeyframe(param.key, clipTime, draft)}
      >
        ◆
      </button>
      {keyframed && (
        <button
          className="kf-btn clear"
          title="Clear keyframes"
          onClick={() => onClearKeyframes(param.key)}
        >
          ✕
        </button>
      )}
    </div>
  );
}

/// Editor for a title (text) clip: content + font/background/position.
function TitleEditor(p: {
  clip: Clip;
  onSetText: (text: string) => void;
  onPreviewText: (text: string) => void;
  onPreview: (key: string, v: number) => void;
  onCommit: (key: string, v: number) => void;
}) {
  const [text, setText] = useState(p.clip.text ?? "");
  useEffect(() => setText(p.clip.text ?? ""), [p.clip.text, p.clip.id]);

  const sliders: { key: string; label: string; min: number; max: number; step: number; def: number }[] = [
    { key: "font_size", label: "Font size", min: 16, max: 160, step: 2, def: 56 },
    { key: "title_bg", label: "Background", min: 0, max: 1, step: 0.05, def: 0.5 },
    { key: "pos_x", label: "Position X", min: -0.5, max: 0.5, step: 0.01, def: 0 },
    { key: "pos_y", label: "Position Y", min: -0.5, max: 0.5, step: 0.01, def: 0.3 },
  ];

  return (
    <>
      <div className="fx-heading">Title</div>
      <div className="fx-panel">
        <textarea
          className="title-text"
          value={text}
          rows={2}
          placeholder="Title text…"
          onChange={(e) => {
            setText(e.target.value);
            p.onPreviewText(e.target.value);
          }}
          onBlur={(e) => p.onSetText(e.currentTarget.value)}
        />
        {sliders.map((s) => (
          <TitleSlider key={s.key} spec={s} clip={p.clip} onPreview={p.onPreview} onCommit={p.onCommit} />
        ))}
      </div>
    </>
  );
}

function TitleSlider(p: {
  spec: { key: string; label: string; min: number; max: number; step: number; def: number };
  clip: Clip;
  onPreview: (key: string, v: number) => void;
  onCommit: (key: string, v: number) => void;
}) {
  const val = p.clip.fx?.[p.spec.key] ?? p.spec.def;
  const [draft, setDraft] = useState(val);
  useEffect(() => setDraft(val), [val]);
  return (
    <div className="fx-row active">
      <span className="fx-label">{p.spec.label}</span>
      <input
        type="range"
        min={p.spec.min}
        max={p.spec.max}
        step={p.spec.step}
        value={draft}
        onChange={(e) => {
          const v = Number(e.target.value);
          setDraft(v);
          p.onPreview(p.spec.key, v);
        }}
        onPointerUp={() => p.onCommit(p.spec.key, draft)}
        onKeyUp={() => p.onCommit(p.spec.key, draft)}
      />
      <span className="fx-val">{draft.toFixed(2)}</span>
    </div>
  );
}

/// Transition INTO the selected clip from its left neighbor.
function TransitionControl(p: { clip: Clip; onSet: (dur: number, dip: boolean) => void }) {
  const dur = fxValue(p.clip, "trans_dur");
  const dip = fxValue(p.clip, "trans_dip") > 0.5;
  const kind = dur < 0.05 ? "none" : dip ? "dip" : "dissolve";
  const [len, setLen] = useState(dur > 0.05 ? dur : 1);
  useEffect(() => {
    if (dur > 0.05) setLen(dur);
  }, [dur]);

  const apply = (k: string, l: number) => {
    if (k === "none") p.onSet(0, false);
    else p.onSet(l, k === "dip");
  };

  return (
    <>
      <div className="fx-heading">Transition</div>
      <div className="fx-panel">
        <Segmented
          options={[
            { value: "none", label: "None" },
            { value: "dissolve", label: "Dissolve" },
            { value: "dip", label: "Dip" },
          ]}
          value={kind}
          onChange={(k) => apply(k, len)}
        />
        <div className={`fx-row${kind === "none" ? "" : " active"}`}>
          <span className="fx-label">Duration</span>
          <input
            type="range"
            min={0.2}
            max={2}
            step={0.1}
            value={len}
            disabled={kind === "none"}
            onChange={(e) => setLen(Number(e.target.value))}
            onPointerUp={() => kind !== "none" && apply(kind, len)}
            onKeyUp={() => kind !== "none" && apply(kind, len)}
          />
          <span className="fx-val">{len.toFixed(1)}s</span>
        </div>
      </div>
    </>
  );
}

/// Retime (speed) control — presets + fine slider. Constant per clip.
function RetimeControl(p: {
  clip: Clip;
  onPreview: (key: string, v: number) => void;
  onCommit: (key: string, v: number) => void;
}) {
  const speed = p.clip.fx?.speed ?? 1;
  const [draft, setDraft] = useState(speed);
  useEffect(() => setDraft(speed), [speed]);
  const presets = [0.25, 0.5, 1, 2, 4];
  return (
    <>
      <div className="fx-heading">Retime</div>
      <div className="fx-panel">
        <div className="speed-presets">
          {presets.map((s) => (
            <button
              key={s}
              className={Math.abs(draft - s) < 0.01 ? "on" : ""}
              onClick={() => {
                setDraft(s);
                p.onCommit("speed", s);
              }}
            >
              {s}×
            </button>
          ))}
        </div>
        <div className="fx-row active">
          <span className="fx-label">Speed</span>
          <input
            type="range"
            min={0.25}
            max={4}
            step={0.05}
            value={draft}
            onChange={(e) => {
              const v = Number(e.target.value);
              setDraft(v);
              p.onPreview("speed", v);
            }}
            onPointerUp={() => p.onCommit("speed", draft)}
            onKeyUp={() => p.onCommit("speed", draft)}
          />
          <span className="fx-val">{draft.toFixed(2)}×</span>
        </div>
      </div>
    </>
  );
}

function EffectControls(p: {
  clip: Clip;
  clipTime: number;
  onPreview: (key: string, v: number) => void;
  onCommit: (key: string, v: number) => void;
  onSetKeyframe: (key: string, t: number, v: number) => void;
  onClearKeyframes: (key: string) => void;
}) {
  return (
    <div className="fx-panel">
      {FX_GROUPS.map((g) => (
        <div className="fx-group" key={g.title}>
          <div className="fx-group-title">{g.title}</div>
          {g.params.map((param) => (
            <FxSlider
              key={param.key}
              param={param}
              clip={p.clip}
              clipTime={p.clipTime}
              onPreview={p.onPreview}
              onCommit={p.onCommit}
              onSetKeyframe={p.onSetKeyframe}
              onClearKeyframes={p.onClearKeyframes}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

/// Numeric field that commits on Enter/blur — the Inspector edit unit.
function NumField(p: {
  label: string;
  value: number;
  onCommit: (v: number) => void;
  min?: number;
}) {
  const [text, setText] = useState(p.value.toFixed(2));
  useEffect(() => setText(p.value.toFixed(2)), [p.value]);
  const commit = () => {
    const v = parseFloat(text);
    if (!Number.isNaN(v) && v !== p.value) p.onCommit(Math.max(p.min ?? -Infinity, v));
    else setText(p.value.toFixed(2));
  };
  return (
    <label className="field">
      <span>{p.label}</span>
      <input
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
    </label>
  );
}

export function Inspector(p: {
  mode: "create" | "studio";
  clip: Clip | null;
  media: Record<string, MediaItem>;
  onMove: (id: string, track: string, start: number) => void;
  onTrim: (id: string, start: number, len: number, srcIn: number) => void;
  onFxPreview: (key: string, v: number) => void;
  onFxCommit: (key: string, v: number) => void;
  clipTime: number;
  onSetKeyframe: (key: string, t: number, v: number) => void;
  onClearKeyframes: (key: string) => void;
  hasLeftNeighbor: boolean;
  onSetTransition: (dur: number, dip: boolean) => void;
  onSetTitleText: (text: string) => void;
  onPreviewTitleText: (text: string) => void;
  // transcript
  words: Word[] | null;
  transcriptMediaName: string | null;
  wordSel: { a: number; b: number } | null;
  isWordCut: (w: Word) => boolean;
  isFiller: (w: Word) => boolean;
  onWordClick: (idx: number, shift: boolean) => void;
  onCutSelection: () => void;
  smartCuts: { fillers: [number, number][]; silences: [number, number][] } | null;
  onCutRanges: (r: [number, number][]) => void;
  silenceGapS: number;
}) {
  const [tab, setTab] = useState<Tab>(p.mode === "create" ? "transcript" : "inspector");
  useEffect(() => {
    if (p.mode === "create") setTab("transcript");
  }, [p.mode]);

  const clipMedia = p.clip ? p.media[p.clip.media] : null;

  return (
    <aside className="panel right-panel">
      {p.mode === "studio" && (
        <Segmented
          options={[
            { value: "inspector", label: "Inspector" },
            { value: "transcript", label: "Transcript" },
          ]}
          value={tab}
          onChange={setTab}
        />
      )}

      {tab === "inspector" && p.mode === "studio" && (
        <div className="inspector-body">
          {!p.clip ? (
            <div className="empty-state">Select a clip to inspect it.</div>
          ) : (
            <>
              <div
                className="insp-header"
                style={{ borderColor: `hsl(${mediaHue(p.clip.media)} 60% 45%)` }}
              >
                {p.clip.name}
              </div>
              <div className="field-grid">
                <NumField
                  label="Start"
                  value={p.clip.start}
                  min={0}
                  onCommit={(v) => p.onMove(p.clip!.id, p.clip!.track, v)}
                />
                <NumField
                  label="Length"
                  value={p.clip.len}
                  min={0.1}
                  onCommit={(v) => p.onTrim(p.clip!.id, p.clip!.start, v, p.clip!.src_in)}
                />
                <NumField
                  label="Source in"
                  value={p.clip.src_in}
                  min={0}
                  onCommit={(v) => p.onTrim(p.clip!.id, p.clip!.start, p.clip!.len, v)}
                />
                <label className="field">
                  <span>Track</span>
                  <input value={p.clip.track} readOnly />
                </label>
              </div>
              {clipMedia && (
                <div className="insp-meta">
                  <div>
                    <span>Media</span>
                    {clipMedia.name}
                  </div>
                  <div>
                    <span>Duration</span>
                    {formatTC(clipMedia.duration_s)}
                  </div>
                  <div>
                    <span>Audio</span>
                    {clipMedia.waveform.length > 0 ? "yes" : "none"}
                  </div>
                </div>
              )}
              {p.clip.text ? (
                <TitleEditor
                  clip={p.clip}
                  onSetText={p.onSetTitleText}
                  onPreviewText={p.onPreviewTitleText}
                  onPreview={p.onFxPreview}
                  onCommit={p.onFxCommit}
                />
              ) : (
                <>
                  <RetimeControl clip={p.clip} onPreview={p.onFxPreview} onCommit={p.onFxCommit} />
                  {p.hasLeftNeighbor && (
                    <TransitionControl clip={p.clip} onSet={p.onSetTransition} />
                  )}
                  <div className="fx-heading">
                    Effect Controls
                    <span className="fx-kf-hint">◆ keyframe @ {p.clipTime.toFixed(1)}s</span>
                  </div>
                  <EffectControls
                    clip={p.clip}
                    clipTime={p.clipTime}
                    onPreview={p.onFxPreview}
                    onCommit={p.onFxCommit}
                    onSetKeyframe={p.onSetKeyframe}
                    onClearKeyframes={p.onClearKeyframes}
                  />
                </>
              )}
            </>
          )}
        </div>
      )}

      {tab === "transcript" && (
        <div className="transcript-body">
          {!p.words ? (
            <div className="empty-state">
              {p.mode === "create"
                ? "Import a clip with speech — it transcribes automatically."
                : "Transcribe a clip from the Media panel to edit it as text."}
            </div>
          ) : (
            <>
              <div className="transcript-hint">
                {p.transcriptMediaName} · click = seek · shift-click = range
              </div>
              <div className="words">
                {p.words.map((w, i) => {
                  const sel = p.wordSel && i >= Math.min(p.wordSel.a, p.wordSel.b) && i <= Math.max(p.wordSel.a, p.wordSel.b);
                  return (
                    <span
                      key={i}
                      className={`word${sel ? " sel" : ""}${p.isWordCut(w) ? " cut" : ""}${p.isFiller(w) ? " filler" : ""}`}
                      onClick={(e) => p.onWordClick(i, e.shiftKey)}
                    >
                      {w.text}
                    </span>
                  );
                })}
              </div>
              {p.wordSel && (
                <button className="danger-btn" onClick={p.onCutSelection}>
                  ✂ Cut {Math.abs(p.wordSel.b - p.wordSel.a) + 1} word
                  {p.wordSel.a === p.wordSel.b ? "" : "s"} from video
                </button>
              )}
              {p.smartCuts && (p.smartCuts.fillers.length > 0 || p.smartCuts.silences.length > 0) && (
                <div className="smart-cuts">
                  {p.smartCuts.fillers.length > 0 && (
                    <button className="smart-btn" onClick={() => p.onCutRanges(p.smartCuts!.fillers)}>
                      ✂ {p.smartCuts.fillers.length} filler word
                      {p.smartCuts.fillers.length === 1 ? "" : "s"}
                    </button>
                  )}
                  {p.smartCuts.silences.length > 0 && (
                    <button className="smart-btn" onClick={() => p.onCutRanges(p.smartCuts!.silences)}>
                      ✂ {p.smartCuts.silences.length} silence
                      {p.smartCuts.silences.length === 1 ? "" : "s"} (≥{p.silenceGapS}s)
                    </button>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </aside>
  );
}
