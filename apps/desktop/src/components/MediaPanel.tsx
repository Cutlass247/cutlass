import { useState } from "react";
import { MediaItem } from "../ipc";
import { EffectPreset, EFFECT_GROUPS, LOOKS } from "../effects";
import { lookStyle, mediaHue, Segmented } from "./ui";

/// A Look button. When a current frame is available it renders as a live
/// thumbnail graded with the look, so you pick by eye.
function LookChip(p: {
  look: EffectPreset;
  frameSrc: string | null;
  disabled: boolean;
  custom?: boolean;
  onClick: () => void;
  onDelete?: () => void;
}) {
  const params = p.look.params ?? {};
  const vig = params.vignette ?? 0;
  if (!p.frameSrc) {
    return (
      <button className="look-chip" title={p.look.desc} disabled={p.disabled} onClick={p.onClick}>
        {p.look.name}
        {p.custom && p.onDelete && (
          <span
            className="look-del"
            onClick={(e) => {
              e.stopPropagation();
              p.onDelete!();
            }}
          >
            ✕
          </span>
        )}
      </button>
    );
  }
  return (
    <button className="look-chip preview" title={p.look.desc} disabled={p.disabled} onClick={p.onClick}>
      <span className="look-thumb">
        <img src={p.frameSrc} alt="" draggable={false} style={lookStyle(params)} />
        {vig > 0.01 && <span className="look-vig" style={{ opacity: Math.min(1, vig) }} />}
        <span className="look-name">{p.look.name}</span>
      </span>
      {p.custom && p.onDelete && (
        <span
          className="look-del"
          onClick={(e) => {
            e.stopPropagation();
            p.onDelete!();
          }}
        >
          ✕
        </span>
      )}
    </button>
  );
}

type Tab = "media" | "effects";

export function MediaPanel(p: {
  media: MediaItem[];
  transcripts: Record<string, unknown[]>;
  transcribing: string | null;
  onImport: () => void;
  onTranscribe: (id: string) => void;
  onMediaPointerDown: (mediaId: string, e: React.PointerEvent) => void;
  onRemoveMedia: (mediaId: string) => void;
  hasSelection: boolean;
  onApplyEffect: (params: Record<string, number>) => void;
  onToggleEffect: (params: Record<string, number>) => void;
  effectActive: (params: Record<string, number>) => boolean;
  onKenBurns: () => void;
  captionsReady: boolean;
  onGenerateCaptions: () => void;
  frameSrc: string | null;
  customLooks: { name: string; params: Record<string, number> }[];
  onSaveLook: () => void;
  onDeleteLook: (name: string) => void;
  selectedLut: string;
  onImportLut: () => void;
  onRemoveLut: () => void;
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
          <div className={`fx-lib-hint${p.hasSelection ? "" : " warn"}`}>
            {p.hasSelection
              ? "Click to apply to the selected clip — fine-tune in the Inspector."
              : "Select a clip on the timeline to apply effects."}
          </div>

          <div className="fx-lib-section">
            <div className="fx-lib-title">Captions</div>
            <button
              className="caption-btn"
              disabled={!p.captionsReady}
              title={
                p.captionsReady
                  ? "Create styled captions from the transcript"
                  : "Transcribe a clip first (Media tab → Transcribe)"
              }
              onClick={p.onGenerateCaptions}
            >
              ✨ Generate captions from transcript
            </button>
            {!p.captionsReady && (
              <div className="fx-lib-note">Transcribe a clip first to enable captions.</div>
            )}
          </div>

          <div className="fx-lib-section">
            <div className="fx-lib-title">Looks</div>
            <div className="look-grid">
              {LOOKS.map((l) => (
                <LookChip
                  key={l.name}
                  look={l}
                  frameSrc={p.frameSrc}
                  disabled={!p.hasSelection}
                  onClick={() => l.params && p.onApplyEffect(l.params)}
                />
              ))}
              {p.customLooks.map((l) => (
                <LookChip
                  key={l.name}
                  look={l}
                  frameSrc={p.frameSrc}
                  disabled={!p.hasSelection}
                  custom
                  onClick={() => p.onApplyEffect(l.params)}
                  onDelete={() => p.onDeleteLook(l.name)}
                />
              ))}
            </div>
            <button
              className="save-look-btn"
              disabled={!p.hasSelection}
              title="Save this clip's colour grade as a reusable Look"
              onClick={p.onSaveLook}
            >
              + Save current grade as Look
            </button>
          </div>

          <div className="fx-lib-section">
            <div className="fx-lib-title">LUT</div>
            {p.selectedLut ? (
              <div className="lut-applied">
                <span className="lut-name" title={p.selectedLut}>
                  {p.selectedLut.split(/[\\/]/).pop()}
                </span>
                <button className="lut-remove" onClick={p.onRemoveLut} title="Remove LUT">
                  ✕
                </button>
              </div>
            ) : (
              <button
                className="save-look-btn"
                disabled={!p.hasSelection}
                title="Load a .cube 3D LUT onto the selected clip"
                onClick={p.onImportLut}
              >
                + Import LUT (.cube)
              </button>
            )}
          </div>

          {EFFECT_GROUPS.map((g) => (
            <div className="fx-lib-section" key={g.title}>
              <div className="fx-lib-title">{g.title}</div>
              <div className="effect-grid">
                {g.items.map((e) => {
                  const on = !e.action && !!e.params && p.effectActive(e.params);
                  return (
                    <button
                      key={e.name}
                      className={`effect-chip${on ? " on" : ""}`}
                      title={e.desc}
                      aria-pressed={on}
                      disabled={!p.hasSelection}
                      onClick={() =>
                        e.action === "kenburns"
                          ? p.onKenBurns()
                          : e.params && p.onToggleEffect(e.params)
                      }
                    >
                      <span className="effect-chip-name">{e.name}</span>
                      <span className="effect-chip-desc">{e.desc}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
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
                <button
                  className="bin-remove"
                  title="Remove from project"
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation();
                    p.onRemoveMedia(m.id);
                  }}
                >
                  ✕
                </button>
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
