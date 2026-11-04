import { useState } from "react";
import { MediaItem } from "../ipc";
import { EffectPreset, EFFECT_GROUPS, LOOKS } from "../effects";
import { lookStyle, mediaHue, Segmented } from "./ui";

/// A sound file has no frame to show, so the bin draws its waveform: the
/// loudest peak in each of a couple of dozen slices, as bars. A file too long
/// to have had its waveform decoded shows a flat line.
function AudioThumb({ peaks }: { peaks: number[] }) {
  const n = 24;
  const bars = Array.from({ length: n }, (_, i) => {
    const a = Math.floor((i / n) * peaks.length);
    const b = Math.max(a + 1, Math.floor(((i + 1) / n) * peaks.length));
    let v = 0;
    for (let j = a; j < b && j < peaks.length; j++) v = Math.max(v, peaks[j]);
    return v;
  });
  return (
    <svg className="bin-audio" viewBox={`0 0 ${n * 3} 40`} preserveAspectRatio="none" aria-hidden="true">
      {bars.map((v, i) => {
        const h = Math.max(2, v * 32);
        return <rect key={i} x={i * 3 + 0.5} y={20 - h / 2} width={2} height={h} rx={1} />;
      })}
    </svg>
  );
}

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
  transcribeProgress: Record<string, number>;
  transcribeMode: "fast" | "private";
  onTranscribeMode: (m: "fast" | "private") => void;
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
          {p.media.length > 0 && (
            <div className="cf-row" style={{ padding: "0 2px 6px" }}>
              <span className="cf-lbl" title="Fast = cloud GPU (seconds); Private = on-device. Neither sends your video; finding the best moments sends the transcript either way.">
                Transcribe
              </span>
              <Segmented
                options={[
                  { value: "fast", label: "Fast" },
                  { value: "private", label: "Private" },
                ]}
                value={p.transcribeMode}
                onChange={(v) => p.onTranscribeMode(v as "fast" | "private")}
              />
            </div>
          )}
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
                data-media-id={m.id}
                title={
                  m.kind === "audio"
                    ? "Drag onto an audio track to place it"
                    : "Drag onto a timeline track to place it"
                }
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
                  {m.kind === "audio" ? (
                    <AudioThumb peaks={m.waveform} />
                  ) : (
                    <img src={m.thumbs[0]} alt="" draggable={false} />
                  )}
                </div>
                <div className="bin-meta-block">
                  <div className="bin-name">{m.name}</div>
                  <div className="bin-meta">
                    {m.kind === "audio"
                      ? `${m.duration_s.toFixed(1)}s · audio file`
                      : `${m.duration_s.toFixed(1)}s · ${m.thumbs.length} frames${m.waveform.length > 0 ? " · audio" : ""}`}
                  </div>
                  {p.transcripts[m.id] ? (
                    <div className="bin-meta ok">📝 transcribed</div>
                  ) : (
                    <button
                      className="mini-btn"
                      disabled={p.transcribing !== null}
                      onClick={() => p.onTranscribe(m.id)}
                    >
                      {p.transcribing === m.id
                        ? `Transcribing… ${Math.round(p.transcribeProgress[m.id] ?? 0)}%`
                        : p.transcribeMode === "fast"
                        ? "⚡ Transcribe"
                        : "Transcribe"}
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
