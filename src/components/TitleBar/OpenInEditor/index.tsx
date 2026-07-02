import { useEffect, useRef, useState } from "react";
import type { DetectedEditor } from "@/api";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { EditorTile } from "./EditorTile";
import { detectEditors, EDITOR_PREF_KEY } from "./editors";

/** Right-side "Open in editor" launcher: a tile of the last-used editor that
 *  opens the active agent's worktree, plus a caret that picks among the editors
 *  actually installed on this machine. Hidden until an agent is open (nothing
 *  to open otherwise) and until at least one editor is detected. */
export function OpenInEditor() {
  const agentId = useActiveAgentId();
  const setLastError = useAppStore((s) => s.setLastError);
  const [editors, setEditors] = useState<DetectedEditor[]>([]);
  const [selectedId, setSelectedId] = useState(() => localStorage.getItem(EDITOR_PREF_KEY));
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    detectEditors().then(setEditors);
  }, []);
  useOutsideClose(ref, open, () => setOpen(false));

  if (!agentId || editors.length === 0) return null;
  const current = editors.find((e) => e.id === selectedId) ?? editors[0];

  const openIn = (id: string) => {
    setOpen(false);
    api.openInEditor(agentId, id).catch((e) => setLastError(String(e)));
  };
  const pick = (id: string) => {
    setSelectedId(id);
    localStorage.setItem(EDITOR_PREF_KEY, id);
    openIn(id);
  };

  return (
    <>
      <div className={`oe ${open ? "open" : ""}`} ref={ref}>
        <button
          type="button"
          className="oe-main tip"
          data-tip-down=""
          data-tip={`Open in ${current.label}`}
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

/** The selected real agent's id, or null when at Home / a draft / settings —
 *  i.e. when there is no worktree to open. */
function useActiveAgentId(): string | null {
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const agent = useAppStore((s) => s.workspace?.agents.find((a) => a.id === selectedId));
  if (activeDraftId || settingsScreenOpen || !agent) return null;
  return agent.id;
}

/** Close the menu on an outside mousedown or Escape while it's open. */
function useOutsideClose(
  ref: React.RefObject<HTMLElement | null>,
  active: boolean,
  close: () => void,
) {
  useEffect(() => {
    if (!active) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [active, ref, close]);
}
