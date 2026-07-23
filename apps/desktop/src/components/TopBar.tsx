import { Kbd, Logo, MenuBarMenu, Segmented } from "./ui";

export type Mode = "create" | "studio";

export function TopBar(p: {
  projectName: string;
  dirty: boolean;
  mode: Mode;
  room: string | null;
  exporting: number | null;
  canEdit: boolean;
  inTauri: boolean;
  onMode: (m: Mode) => void;
  onImport: () => void;
  onOpen: () => void;
  onSave: () => void;
  onExport: () => void;
  onUndo: () => void;
  onRedo: () => void;
  onSplit: () => void;
  onAddTitle: () => void;
  onDeleteSel: (ripple: boolean) => void;
  onCollab: () => void;
  onZoom: (dir: 1 | -1) => void;
  hasSelection: boolean;
  onFeedback: () => void;
}) {
  return (
    <header className="topbar">
      <span className="logo" title="Cutlass">
        <Logo size={22} />
      </span>
      <nav className="menubar">
        <MenuBarMenu
          title="File"
          items={[
            { label: "Import media…", onSelect: p.onImport },
            { separator: true, label: "" },
            { label: "Open project…", hint: "Ctrl+O", onSelect: p.onOpen },
            { label: "Save project", hint: "Ctrl+S", onSelect: p.onSave },
            { separator: true, label: "" },
            { label: "Export…", onSelect: p.onExport },
          ]}
        />
        <MenuBarMenu
          title="Edit"
          items={[
            { label: "Undo", hint: "Ctrl+Z", onSelect: p.onUndo },
            { label: "Redo", hint: "Ctrl+Y", onSelect: p.onRedo },
            { separator: true, label: "" },
            { label: "Add title at playhead", onSelect: p.onAddTitle },
            { label: "Split at playhead", hint: "S", onSelect: p.onSplit },
            {
              label: "Delete clip (lift)",
              hint: "Del",
              disabled: !p.hasSelection,
              onSelect: () => p.onDeleteSel(false),
            },
            {
              label: "Ripple delete clip",
              hint: "Shift+Del",
              disabled: !p.hasSelection,
              onSelect: () => p.onDeleteSel(true),
            },
          ]}
        />
        <MenuBarMenu
          title="View"
          items={[
            { label: "Zoom in", hint: "Ctrl+Wheel", onSelect: () => p.onZoom(1) },
            { label: "Zoom out", onSelect: () => p.onZoom(-1) },
            { separator: true, label: "" },
            {
              label: p.mode === "create" ? "Switch to Studio" : "Switch to Create",
              onSelect: () => p.onMode(p.mode === "create" ? "studio" : "create"),
            },
          ]}
        />
        <MenuBarMenu
          title="Help"
          items={[
            { label: "Send beta feedback…", onSelect: p.onFeedback },
            { separator: true, label: "" },
            { label: "Cutlass 0.1.0 (beta)", disabled: true, onSelect: () => {} },
          ]}
        />
      </nav>

      <div className="title-block">
        <span className="project-name">{p.projectName}</span>
        <span className={`save-dot${p.dirty ? " dirty" : ""}`}>
          {p.dirty ? "● Unsaved" : "Saved"}
        </span>
        {p.room && <span className="badge live">🔗 {p.room}</span>}
      </div>

      <Segmented
        options={[
          { value: "create", label: "Create" },
          { value: "studio", label: "Studio" },
        ]}
        value={p.mode}
        onChange={p.onMode}
      />
      {p.mode === "create" && (
        <button className="tool-btn wide" onClick={p.onImport}>
          + Import
        </button>
      )}
      {p.mode === "studio" && (
        <div className="topbar-actions">
          <button className="tool-btn" title="Undo (Ctrl+Z)" onClick={p.onUndo}>
            ↩
          </button>
          <button className="tool-btn" title="Redo (Ctrl+Y)" onClick={p.onRedo}>
            ↪
          </button>
          {!p.room && (
            <button className="tool-btn wide" onClick={p.onCollab} disabled={!p.inTauri}>
              Collab
            </button>
          )}
        </div>
      )}
      <button
        className="export-btn"
        onClick={p.onExport}
        disabled={p.exporting !== null || !p.canEdit}
        title="Export the timeline…"
      >
        {p.exporting !== null ? `Exporting ${Math.round(p.exporting * 100)}%` : "Export"}
        {p.exporting === null && <Kbd>MP4</Kbd>}
      </button>
    </header>
  );
}
