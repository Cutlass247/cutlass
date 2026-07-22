import { useState } from "react";
import { MediaItem } from "../ipc";
import { EFFECTS, LOOKS } from "../effects";
import { mediaHue, Segmented } from "./ui";

type Tab = "media" | "effects";

export function MediaPanel(p: {
  media: MediaItem[];
  transcripts: Record<string, unknown[]>;
  transcribing: string | null;
  onImport: () => void;
  onTranscribe: (id: string) => void;
  onMediaPointerDown: (mediaId: string, e: React.PointerEvent) => void;
  hasSelection: boolean;
  onApplyEffect: (params: Record<string, number>) => void;
  busy: boolean;
}) {
  const [tab, setTab] = useState<Tab>("media");
  const [query, setQuery] = useState("");
  const filtered = p.media.filter((m) =>
    m.name.toLowerCase().includes(query.trim().toLowerCase())
  );

  return (
    <aside className="panel left-panel">
      <Segmented
        options={[
          { value: "media", label: "Media" },
          { value: "effects", label: "Effects" },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === "effects" ? (
        <div className="fx-library">
          <div className="fx-lib-hint">
            {p.hasSelection
              ? "Click a Look or effect to apply it to the selected clip — tune it in the Inspector."
              : "Select a clip on the timeline, then apply a Look or effect."}
          </div>
          <div className="fx-lib-title">Looks</div>
          <div className="look-grid">
            {LOOKS.map((l) => (
              <button
                key={l.name}
                className="look-chip"
                title={l.desc}
                disabled={!p.hasSelection}
                onClick={() => p.onApplyEffect(l.params)}
              >
                {l.name}
              </button>
            ))}
          </div>
          <div className="fx-lib-title">Effects</div>
          <div className="effect-list">
            {EFFECTS.map((e) => (
              <button
                key={e.name}
                className="effect-row"
                disabled={!p.hasSelection}
                onClick={() => p.onApplyEffect(e.params)}
              >
                <span className="effect-name">{e.name}</span>
                <span className="effect-desc">{e.desc}</span>
              </button>
            ))}
          </div>
        </div>
      ) : (
        <>
          <input
            className="search"
            placeholder="Search media…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <div className="panel-btn-row">
            <button className="primary-btn" onClick={p.onImport} disabled={p.busy}>
              {p.busy ? "Importing…" : "+ Import media"}
            </button>
          </div>
          <div className="bin-list">
            {filtered.length === 0 && (
              <div className="empty-state">
                {p.media.length === 0
                  ? "Import a video to begin. Scrubbing runs on generated proxies — your source files are never touched."
                  : "No media matches the search."}
              </div>
            )}
            {filtered.map((m) => (
              <div
                className="bin-item"
                key={m.id}
                title="Drag onto a timeline track to place it"
                onPointerDown={(e) => p.onMediaPointerDown(m.id, e)}
              >
                <div
                  className="bin-thumb"
                  style={{ borderColor: `hsl(${mediaHue(m.id)} 60% 45%)` }}
                >
                  <img src={m.thumbs[0]} alt="" draggable={false} />
                </div>
                <div className="bin-meta-block">
                  <div className="bin-name">{m.name}</div>
                  <div className="bin-meta">
                    {m.duration_s.toFixed(1)}s · {m.thumbs.length} frames
                    {m.waveform.length > 0 ? " · audio" : ""}
                  </div>
                  {p.transcripts[m.id] ? (
                    <div className="bin-meta ok">📝 transcribed</div>
                  ) : (
                    <button
                      className="mini-btn"
                      disabled={p.transcribing !== null}
                      onClick={() => p.onTranscribe(m.id)}
                    >
                      {p.transcribing === m.id ? "Transcribing…" : "Transcribe"}
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
        </>
      )}
    </aside>
  );
}
