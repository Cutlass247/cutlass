import { useState } from "react";
import { MEDIA_DND_TYPE, MediaItem } from "../ipc";
import { mediaHue, Segmented } from "./ui";

type Tab = "media" | "effects";

export function MediaPanel(p: {
  media: MediaItem[];
  transcripts: Record<string, unknown[]>;
  transcribing: string | null;
  onImport: () => void;
  onAddTitle: () => void;
  onTranscribe: (id: string) => void;
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
        <div className="empty-state">
          Effects, transitions and titles land after alpha.
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
            <button className="primary-btn ghost" onClick={p.onAddTitle} title="Add a title on V2 at the playhead">
              + Title
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
                draggable
                title="Drag onto a timeline track to place it"
                onDragStart={(e) => {
                  e.dataTransfer.setData(MEDIA_DND_TYPE, m.id);
                  e.dataTransfer.effectAllowed = "copy";
                }}
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
