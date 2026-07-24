import { useEffect, useState } from "react";
import { ExportOptions, pickExportDir } from "../ipc";
import { RESOLUTIONS } from "./Monitor";

const FORMATS = [
  { value: "mp4_h264", label: "MP4 · H.264 (universal)", ext: "mp4" },
  { value: "mp4_h265", label: "MP4 · H.265 / HEVC (needs GPU support)", ext: "mp4" },
  { value: "mov_prores", label: "MOV · ProRes (master)", ext: "mov" },
  { value: "webm_vp9", label: "WebM · VP9 (web)", ext: "webm" },
];
const extOf = (fmt: string) => FORMATS.find((f) => f.value === fmt)?.ext ?? "mp4";

const FPS_OPTIONS = [24, 30, 60];
const QUALITIES = [
  { value: "low", label: "Low — smaller file" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High — best quality" },
];

interface PresetDef {
  format: string;
  res: string;
  fps: number;
  quality: string;
}
const PRESETS: { name: string; def: PresetDef | null }[] = [
  { name: "YouTube 1080p", def: { format: "mp4_h264", res: "1080p", fps: 30, quality: "high" } },
  { name: "YouTube 4K", def: { format: "mp4_h264", res: "4K", fps: 30, quality: "high" } },
  { name: "Web — small", def: { format: "mp4_h264", res: "720p", fps: 30, quality: "medium" } },
  { name: "HEVC 1080p", def: { format: "mp4_h265", res: "1080p", fps: 30, quality: "high" } },
  { name: "ProRes master", def: { format: "mov_prores", res: "1080p", fps: 30, quality: "high" } },
  { name: "WebM", def: { format: "webm_vp9", res: "1080p", fps: 30, quality: "medium" } },
  { name: "Custom", def: null },
];

export function ExportDialog(p: {
  initialDir: string;
  /// tallest source in the project (0 = unknown). Exporting above this
  /// upscales, which measurably *lowers* quality and inflates the file.
  sourceHeight?: number;
  onCancel: () => void;
  onExport: (opts: ExportOptions) => void;
}) {
  const [name, setName] = useState("export");
  const [dir, setDir] = useState(p.initialDir);
  const [preset, setPreset] = useState("YouTube 1080p");
  const [format, setFormat] = useState("mp4_h264");
  // default to the resolution that matches the footage, so the common path
  // never silently upscales
  const [resLabel, setResLabel] = useState(() => {
    const sh = p.sourceHeight ?? 0;
    if (!sh) return "1080p";
    const fit = [...RESOLUTIONS].reverse().find((r) => r.h <= sh);
    return (fit ?? RESOLUTIONS[0]).label;
  });
  const [fps, setFps] = useState(30);
  const [quality, setQuality] = useState("high");

  useEffect(() => setDir(p.initialDir), [p.initialDir]);

  const applyPreset = (n: string) => {
    setPreset(n);
    const def = PRESETS.find((x) => x.name === n)?.def;
    if (def) {
      setFormat(def.format);
      setResLabel(def.res);
      setFps(def.fps);
      setQuality(def.quality);
    }
  };
  // wrap a setter so any manual field change flips the preset to Custom
  const manual =
    <T,>(setter: (v: T) => void) =>
    (v: T) => {
      setter(v);
      setPreset("Custom");
    };

  const res = RESOLUTIONS.find((r) => r.label === resLabel) ?? RESOLUTIONS[1];
  const ext = extOf(format);
  const sep = dir.includes("\\") ? "\\" : "/";
  const fullPath = `${dir}${dir && !dir.endsWith(sep) ? sep : ""}${name || "export"}.${ext}`;

  const browse = async () => {
    const d = await pickExportDir();
    if (d) setDir(d);
  };
  const submit = () =>
    p.onExport({ path: fullPath, width: res.w, height: res.h, fps, format, quality });

  return (
    <div className="modal-overlay" onPointerDown={p.onCancel}>
      <div className="modal export-dialog" onPointerDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Export settings</div>

        <label className="ex-field">
          <span>Preset</span>
          <select value={preset} onChange={(e) => applyPreset(e.target.value)}>
            {PRESETS.map((x) => (
              <option key={x.name} value={x.name}>
                {x.name}
              </option>
            ))}
          </select>
        </label>

        <label className="ex-field">
          <span>File name</span>
          <div className="ex-name">
            <input value={name} onChange={(e) => setName(e.target.value)} placeholder="export" />
            <span className="ex-ext">.{ext}</span>
          </div>
        </label>

        <label className="ex-field">
          <span>Location</span>
          <div className="ex-name">
            <input value={dir} onChange={(e) => setDir(e.target.value)} placeholder="Choose a folder…" />
            <button className="ghost-btn small" onClick={browse}>
              Browse…
            </button>
          </div>
        </label>

        <div className="ex-grid">
          <label className="ex-field">
            <span>Format</span>
            <select value={format} onChange={(e) => manual(setFormat)(e.target.value)}>
              {FORMATS.map((f) => (
                <option key={f.value} value={f.value}>
                  {f.label}
                </option>
              ))}
            </select>
          </label>
          <label className="ex-field">
            <span>Resolution</span>
            <select value={resLabel} onChange={(e) => manual(setResLabel)(e.target.value)}>
              {RESOLUTIONS.map((r) => (
                <option key={r.label} value={r.label}>
                  {r.label} ({r.w}×{r.h})
                  {!!p.sourceHeight && r.h > p.sourceHeight ? " — upscaled" : ""}
                </option>
              ))}
            </select>
            {!!p.sourceHeight && res.h > p.sourceHeight && (
              <span className="ex-warn">
                ⚠ Your footage is {p.sourceHeight}p. Exporting at {res.h}p can’t add
                detail — it looks softer and the file gets much bigger. {p.sourceHeight}p
                is the sharper choice.
              </span>
            )}
          </label>
          <label className="ex-field">
            <span>Frame rate</span>
            <select value={fps} onChange={(e) => manual(setFps)(Number(e.target.value))}>
              {FPS_OPTIONS.map((f) => (
                <option key={f} value={f}>
                  {f} fps
                </option>
              ))}
            </select>
          </label>
          <label className="ex-field">
            <span>Quality</span>
            <select value={quality} onChange={(e) => manual(setQuality)(e.target.value)}>
              {QUALITIES.map((q) => (
                <option key={q.value} value={q.value}>
                  {q.label}
                </option>
              ))}
            </select>
          </label>
        </div>

        <div className="export-path" title={fullPath}>
          {fullPath}
        </div>

        <div className="modal-actions">
          <button className="ghost-btn" onClick={p.onCancel}>
            Cancel
          </button>
          <button className="primary-action" onClick={submit} disabled={!dir.trim() || !name.trim()}>
            Export
          </button>
        </div>
      </div>
    </div>
  );
}
