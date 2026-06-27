import { useState } from "react";
import { Icon, type IconName } from "../../Icon";
import type { ActionTone } from "../primaryActions";
import { Spinner } from "./shared";

/** One action as presented by the split button — the unified shape the main
 *  button, the menu, and the dispatcher all key off. */
export interface SplitActionItem {
  key: string;
  label: string;
  icon: IconName;
  kbd?: string;
}

// ── Split action button ───────────────────────────────────────────
// A split button with a *selectable* default: the main button shows the
// currently-selected action and runs it on click; the caret opens a menu of
// every action for this state. Picking a menu item only changes which action
// the main button will perform — it does NOT execute. The state's primary is
// tagged "default"; the active selection is highlighted. The menu opens
// upward, since the button is pinned to the panel footer.
export function SplitAction({
  items,
  selectedKey,
  tone,
  mainDisabled,
  busyLabel,
  onSelect,
  onRun,
}: {
  items: SplitActionItem[];
  selectedKey: string;
  tone: ActionTone;
  mainDisabled: boolean;
  busyLabel: string | null;
  onSelect: (key: string) => void;
  onRun: () => void;
}) {
  const [open, setOpen] = useState(false);
  const selected = items.find((a) => a.key === selectedKey) ?? items[0];
  const hasMenu = items.length > 1;
  const busy = busyLabel != null;
  if (!selected) return null;

  const toneClass = tone !== "accent" ? tone : "";

  return (
    <div className={`git-split ${toneClass} ${busy ? "busy" : ""}`}>
      <button className="gsa-main" disabled={mainDisabled || busy} onClick={onRun}>
        {busy ? <Spinner /> : <Icon name={selected.icon} />}
        <span className="gsa-label">{busy ? busyLabel : selected.label}</span>
      </button>
      {hasMenu && (
        <button
          className="gsa-caret tip"
          data-tip="Choose action"
          aria-label="Choose action"
          disabled={busy}
          onClick={() => setOpen((v) => !v)}
        >
          <Icon name="chevU" />
        </button>
      )}
      {open && (
        <>
          <div
            style={{ position: "fixed", inset: 0, zIndex: 199 }}
            onClick={() => setOpen(false)}
          />
          <div className="dd gsa-menu">
            {items.map((a) => (
              <div
                key={a.key}
                className={`dd-item ${a.key === selectedKey ? "active" : ""}`}
                onClick={() => {
                  onSelect(a.key);
                  setOpen(false);
                }}
              >
                <div className="di-i">
                  <Icon name={a.icon} size={12} />
                </div>
                <span className="di-l">{a.label}</span>
                {a.kbd && <span className="di-m">{a.kbd}</span>}
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
