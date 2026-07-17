import { useEffect, useState } from "react";
import { Clip, MediaItem, Word } from "../ipc";
import { formatTC, mediaHue, Segmented } from "./ui";

type Tab = "inspector" | "transcript";

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
