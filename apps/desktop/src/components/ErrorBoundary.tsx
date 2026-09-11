import React from "react";
import { saveRecoveryCopy } from "../ipc";

/// Async failures that never reached a `.catch` — kept here so the crash
/// screen can show what was already going wrong before the render threw.
/// A crash is usually the second symptom, not the first.
const recent: string[] = [];

export function noteBackgroundError(what: string) {
  recent.push(`${new Date().toLocaleTimeString()}  ${what}`);
  if (recent.length > 8) recent.shift();
}

/// How many times this window has crashed since it was launched. A second
/// crash means reloading is likely to land straight back here — usually
/// because something in the project itself is what the UI can't render — so
/// the advice has to change.
function crashCount(): number {
  try {
    const n = Number(sessionStorage.getItem("cutlassCrashes") || "0") + 1;
    sessionStorage.setItem("cutlassCrashes", String(n));
    return n;
  } catch {
    return 1;
  }
}

interface State {
  error: Error | null;
  info: string;
  crashes: number;
  savedTo: string | null;
  saveFailed: string | null;
  saving: boolean;
  copied: boolean;
}

/// The last line between a React error and a blank white window.
///
/// Without this, one throw anywhere in the tree unmounts everything: the user
/// gets an empty window with no message, no way to report it, and no way to
/// save an hour of edits. That is the worst outcome the app can produce, and
/// it is entirely avoidable — the project document lives in the Rust backend,
/// not in React, so the work is still there to be written out even when the
/// interface is gone.
///
/// Everything below is deliberately self-contained: plain inline-ish styles
/// through one stylesheet class, no app components, no context, no hooks. It
/// runs in a process that has just proven it can fail, so it must not depend
/// on anything that might be what failed.
export class ErrorBoundary extends React.Component<{ children: React.ReactNode }, State> {
  state: State = {
    error: null,
    info: "",
    crashes: 1,
    savedTo: null,
    saveFailed: null,
    saving: false,
    copied: false,
  };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // Still worth printing: `npm run tauri dev` and a devtools build both show
    // it, which is where this will actually be diagnosed.
    console.error("Cutlass UI crashed:", error, info.componentStack);
    this.setState({ info: (info.componentStack || "").trim(), crashes: crashCount() });
  }

  private save = async () => {
    this.setState({ saving: true, saveFailed: null });
    try {
      const path = await saveRecoveryCopy();
      this.setState({ savedTo: path, saving: false });
    } catch (e) {
      this.setState({ saveFailed: String(e), saving: false });
    }
  };

  private details(): string {
    const { error, info } = this.state;
    const headline = `${error?.name ?? "Error"}: ${error?.message ?? "unknown"}`;
    // V8's `stack` already starts with "Name: message", so printing both
    // repeats the one line a reader looks at first.
    const stack = error?.stack ?? "";
    const body = stack.startsWith(headline) ? stack : `${headline}\n${stack}`;
    return [
      `Cutlass ${__APP_VERSION__} — UI crash`,
      `when: ${new Date().toISOString()}`,
      "",
      body,
      info ? `\ncomponent stack:${info}` : "",
      recent.length ? `\nbefore the crash:\n${recent.join("\n")}` : "",
    ].join("\n");
  }

  private copy = async () => {
    try {
      await navigator.clipboard.writeText(this.details());
      this.setState({ copied: true });
    } catch {
      this.setState({ copied: false });
    }
  };

  render() {
    const { error, crashes, savedTo, saveFailed, saving, copied } = this.state;
    if (!error) return this.props.children;

    const repeat = crashes > 1;
    return (
      <div className="crash">
        <div className="crash-card">
          <h1>Cutlass hit a problem and stopped drawing.</h1>
          <p className="crash-lede">
            Your project is still open in the background — it lives outside the part that
            failed, so nothing has been lost yet. Save a copy of it before doing anything
            else.
          </p>

          <div className="crash-actions">
            <button className="crash-btn primary" onClick={this.save} disabled={saving || !!savedTo}>
              {saving ? "Saving…" : savedTo ? "Copy saved" : "Save a recovery copy"}
            </button>
            <button className="crash-btn" onClick={() => window.location.reload()}>
              {repeat ? "Try reloading anyway" : "Reload Cutlass"}
            </button>
            <button className="crash-btn quiet" onClick={this.copy}>
              {copied ? "Details copied" : "Copy error details"}
            </button>
          </div>

          {savedTo && (
            <p className="crash-saved">
              Saved to <code>{savedTo}</code>
            </p>
          )}
          {saveFailed && (
            <p className="crash-failed">
              The recovery copy couldn&apos;t be written: {saveFailed}
              <br />
              Don&apos;t close this window — try reloading, then save normally.
            </p>
          )}

          <p className="crash-advice">
            {repeat
              ? "This is the second time since Cutlass started, so reloading will probably land back here — something in the project itself is likely what it can't draw. Save the copy, then restart Cutlass and open it again."
              : "Reloading restarts the interface and reopens the project where you left it."}
          </p>

          <details className="crash-details">
            <summary>What went wrong</summary>
            <pre>{this.details()}</pre>
          </details>
          <p className="crash-foot">
            Please send those details to Isaiah — a crash report with the stack in it is
            worth ten without.
          </p>
        </div>
      </div>
    );
  }
}
