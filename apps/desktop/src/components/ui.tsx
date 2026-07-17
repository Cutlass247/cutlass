// Shared UI primitives for the Cutlass shell. Presentation only — no
// editor logic lives here.
import { ReactNode, useEffect, useRef, useState } from "react";

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

export function formatTC(t: number, fps = 30): string {
  const m = Math.floor(t / 60);
  const s = Math.floor(t % 60);
  const f = Math.floor((t % 1) * fps);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(f).padStart(2, "0")}`;
}
