// Shared UI primitives for the Cutlass shell. Presentation only — no
// editor logic lives here.
import { ReactNode, useEffect, useRef, useState } from "react";

/// The Cutlass mark: a play button sliced by the blade.
export function Logo({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 256 256" aria-label="Cutlass">
      <defs>
        <linearGradient id="lgTri" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="1" stopColor="#c3c8d1" />
        </linearGradient>
        <linearGradient id="lgBlade" x1="0" y1="1" x2="1" y2="0">
          <stop offset="0" stopColor="#c8941f" />
          <stop offset="0.5" stopColor="#f6d06c" />
          <stop offset="1" stopColor="#c8941f" />
        </linearGradient>
      </defs>
      <polygon
        points="93,173 93,189 189,133 153.6,112.4"
        fill="url(#lgTri)"
        stroke="url(#lgTri)"
        strokeWidth="7"
        strokeLinejoin="round"
        opacity="0.9"
      />
      <polygon
        points="83,67 83,163 143.6,102.4"
        fill="url(#lgTri)"
        stroke="url(#lgTri)"
        strokeWidth="7"
        strokeLinejoin="round"
      />
      <line
        x1="58"
        y1="198"
        x2="198"
        y2="58"
        stroke="url(#lgBlade)"
        strokeWidth="12"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function IconButton({
  label,
  hint,
  onClick,
  disabled,
  active,
  danger,
  children,
}: {
  label: string;
  hint?: string;
  onClick?: () => void;
  disabled?: boolean;
  active?: boolean;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      className={`icon-btn${active ? " active" : ""}${danger ? " danger" : ""}`}
      title={hint ? `${label} (${hint})` : label}
      aria-label={label}
      onClick={onClick}
      disabled={disabled}
    >
      {children}
    </button>
  );
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
}) {
  return (
    <div className="seg" role="tablist">
      {options.map((o) => (
        <button
          key={o.value}
          role="tab"
          className={value === o.value ? "on" : ""}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Switch({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="switch" title={label}>
      <span className="switch-label">{label}</span>
      <button
        role="switch"
        aria-checked={checked}
        className={`switch-track${checked ? " on" : ""}`}
        onClick={() => onChange(!checked)}
      >
        <span className="switch-thumb" />
      </button>
    </label>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

export interface MenuAction {
  label: string;
  hint?: string;
  disabled?: boolean;
  onSelect?: () => void;
  separator?: boolean;
}

export function MenuBarMenu({ title, items }: { title: string; items: MenuAction[] }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, [open]);
  return (
    <div className="menu" ref={ref}>
      <button className={`menu-title${open ? " open" : ""}`} onClick={() => setOpen((o) => !o)}>
        {title}
      </button>
      {open && (
        <div className="menu-pop" role="menu">
          {items.map((it, i) =>
            it.separator ? (
              <div className="menu-sep" key={i} />
            ) : (
              <button
                key={i}
                role="menuitem"
                className="menu-item"
                disabled={it.disabled}
                onClick={() => {
                  setOpen(false);
                  it.onSelect?.();
                }}
              >
                <span>{it.label}</span>
                {it.hint && <Kbd>{it.hint}</Kbd>}
              </button>
            )
          )}
        </div>
      )}
    </div>
  );
}

/// Drag handle for resizable panels. Reports deltas; owner clamps.
export function Resizer({
  direction,
  onDelta,
}: {
  direction: "h" | "v";
  onDelta: (px: number) => void;
}) {
  const last = useRef(0);
  return (
    <div
      className={`resizer ${direction}`}
      onPointerDown={(e) => {
        last.current = direction === "h" ? e.clientX : e.clientY;
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* keep dragging while hovered */
        }
      }}
      onPointerMove={(e) => {
        if (!(e.buttons & 1)) return;
        const cur = direction === "h" ? e.clientX : e.clientY;
        onDelta(cur - last.current);
        last.current = cur;
      }}
    />
  );
}

/// Deterministic clip color from its media id.
export function mediaHue(mediaId: string): number {
  let h = 0;
  for (const c of mediaId) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}

/// Approximate a clip's Effect Controls as CSS for the live preview.
/// Export is the pixel-accurate path (ffmpeg); this just has to feel
/// right while scrubbing. `playhead` is absolute timeline seconds.
export function fxStyle(
  clip: { start: number; len: number; fx?: Record<string, number> } | null,
  playhead: number
): React.CSSProperties {
  if (!clip) return {};
  const v = (k: string, d: number) => clip.fx?.[k] ?? d;
  const scale = v("scale", 1);
  const rot = v("rot", 0);
  const px = v("pos_x", 0) * 100;
  const py = v("pos_y", 0) * 100;
  const brightness = 1 + v("brightness", 0); // additive → multiplicative approx
  const contrast = v("contrast", 1);
  const saturation = v("saturation", 1);

  const inClip = playhead - clip.start;
  const fi = v("fade_in", 0);
  const fo = v("fade_out", 0);
  const tr = v("trans_dur", 0); // incoming transition reads as a fade-in
  let opacity = 1;
  const rampIn = Math.max(fi, tr);
  if (rampIn > 0 && inClip < rampIn) opacity = Math.max(0, inClip / rampIn);
  if (fo > 0 && inClip > clip.len - fo)
    opacity = Math.min(opacity, Math.max(0, (clip.len - inClip) / fo));

  return {
    transform: `translate(${px}%, ${py}%) scale(${scale}) rotate(${rot}deg)`,
    filter: `brightness(${brightness}) contrast(${contrast}) saturate(${saturation})`,
    opacity,
  };
}

export function formatTC(t: number, fps = 30): string {
  const m = Math.floor(t / 60);
  const s = Math.floor(t % 60);
  const f = Math.floor((t % 1) * fps);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(f).padStart(2, "0")}`;
}
