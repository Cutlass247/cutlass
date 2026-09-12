import { useEffect, useRef, useState } from "react";

/// A one-field prompt, in the app's own dialog.
///
/// Replaces `window.prompt`, which had two problems worth fixing together.
/// It is a native dialog the app cannot style, so it looks like nothing else
/// in Cutlass; and whether a webview shows one at all is up to the host, so a
/// feature built on it can work in the browser during development and be dead
/// in the shipped build with nothing to show for it. Naming a Look was one
/// such feature.
export interface AskTextProps {
  title: string;
  /// Optional line under the title, for anything the field can't say itself.
  sub?: string;
  initial?: string;
  placeholder?: string;
  confirmLabel?: string;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}

export function AskText(p: AskTextProps) {
  const [value, setValue] = useState(p.initial ?? "");
  const input = useRef<HTMLInputElement>(null);

  // Focus and select, the way a rename field behaves everywhere else: the
  // suggested name is usually right, and when it isn't the user types over it.
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  const trimmed = value.trim();
  const submit = () => {
    if (trimmed) p.onConfirm(trimmed);
  };

  return (
    <div className="modal-overlay" onPointerDown={p.onCancel}>
      <div className="modal" onPointerDown={(e) => e.stopPropagation()}>
        <div className="modal-title">{p.title}</div>
        {p.sub && <div className="modal-sub">{p.sub}</div>}
        <input
          ref={input}
          className="ask-input"
          value={value}
          placeholder={p.placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            // Enter and Escape are what people reach for in a one-field
            // dialog, and the native prompt this replaces handled both.
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              p.onCancel();
            }
          }}
        />
        <div className="modal-actions">
          <button className="ghost-btn" onClick={p.onCancel}>
            Cancel
          </button>
          <button className="primary-action" onClick={submit} disabled={!trimmed}>
            {p.confirmLabel ?? "OK"}
          </button>
        </div>
      </div>
    </div>
  );
}
