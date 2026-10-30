import { useEffect, useRef, useState } from "react";
import type { AgentRecord, DetectedEditor } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { surfaceAgent } from "@/store/surface";
import { useDismiss } from "@/util/hooks";
import { useShortcutKeys } from "@/util/shortcuts";
import { EditorTile } from "./EditorTile";
import { detectEditors, EDITOR_PREF_KEY, editorsFor, inBox, openEditor } from "./editors";

/** Right-side "Open in editor" launcher: a tile of the last-used editor that
 *  opens the active agent's checkout, plus a caret that picks among the editors
 *  actually installed on this machine. Hidden until an agent is open (nothing
 *  to open otherwise) and until at least one editor is detected. A box
 *  workspace offers only the editors that can open it over ssh; with none
 *  installed the tile stays, disabled, saying which would. */
export function OpenInEditor() {
  const agent = useActiveAgent();
  const setLastError = useAppStore((s) => s.setLastError);
  const [detected, setDetected] = useState<DetectedEditor[]>([]);
  const [selectedId, setSelectedId] = useState(() => localStorage.getItem(EDITOR_PREF_KEY));
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const keys = useShortcutKeys("openInEditor");

  useEffect(() => {
    detectEditors().then(setDetected);
  }, []);
  useDismiss(ref, open, () => setOpen(false));

  if (!agent || detected.length === 0) return null;
  const editors = editorsFor(agent, detected);
  const current = editors.find((e) => e.id === selectedId) ?? editors[0];

  if (!current) {
    return (
      <>
        <div className="oe">
          <button
            type="button"
            className="oe-main tip"
            data-tip="This workspace runs in a box; VS Code, Cursor or Windsurf can open it over ssh"
            disabled
          >
            <Icon name="code" size={16} />
          </button>
        </div>
        <span className="tb-vdiv" />
      </>
    );
  }

  const openIn = (id: string) => {
    setOpen(false);
    openEditor(agent, id).catch((e) => setLastError(String(e)));
  };
  const pick = (id: string) => {
    setSelectedId(id);
    localStorage.setItem(EDITOR_PREF_KEY, id);
    openIn(id);
  };
  const tip = inBox(agent) ? `Open the box in ${current.label}` : `Open in ${current.label}`;

  return (
    <>
      <div className={`oe ${open ? "open" : ""}`} ref={ref}>
        <button
          type="button"
          className="oe-main tip"
          data-tip={`${tip} (${keys})`}
          onClick={() => openIn(current.id)}
        >
          <EditorTile editor={current} />
        </button>
        <button
          type="button"
          className="oe-caret"
          aria-label="Choose editor"
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevD" size={13} />
        </button>
        {open && <Menu editors={editors} currentId={current.id} onPick={pick} />}
      </div>
      <span className="tb-vdiv" />
    </>
  );
}

/** The editor picker, split into "Editors" and "Terminals" groups (only the
 *  headers show when both kinds are present — a flat list otherwise). */
function Menu({
  editors,
  currentId,
  onPick,
}: {
  editors: DetectedEditor[];
  currentId: string;
  onPick: (id: string) => void;
}) {
  const groups = [
    { heading: "Editors", items: editors.filter((e) => e.kind === "editor") },
    { heading: "Terminals", items: editors.filter((e) => e.kind === "terminal") },
  ].filter((g) => g.items.length > 0);
  const showHeadings = groups.length > 1;

  return (
    <div className="oe-menu" role="menu">
      {groups.map((g) => (
        <div key={g.heading} className="oe-group">
          <div className="oe-menu-h">{showHeadings ? g.heading : "Open in"}</div>
          {g.items.map((e) => (
            <button
              key={e.id}
              type="button"
              className={`oe-item ${e.id === currentId ? "on" : ""}`}
              role="menuitem"
              onClick={() => onPick(e.id)}
            >
              <EditorTile editor={e} size={20} />
              <span className="oe-item-label">{e.label}</span>
              {e.id === currentId && <Icon name="check" size={12} className="oe-check" />}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}

/** The agent in front of the user, or null while anything else is (Home, a
 *  draft, a run, a full-screen surface) — i.e. when there is no checkout to
 *  open. */
function useActiveAgent(): AgentRecord | null {
  return useAppStore((s) => surfaceAgent(s));
}
