import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { Icon } from "@/components/Icon";
import type { ActionTone } from "@/components/RightPanel/primaryActions";
import {
  DropdownItem,
  DropdownMenu,
  DropdownSection,
  DropdownSeparator,
} from "@/components/ui/Dropdown";
import type { FooterAction, FooterGroup } from "./footerModel";
import { Spinner } from "./shared";

// ── Split action button ───────────────────────────────────────────
// The main button is the ladder's next step and runs it. The caret opens the
// rest of what the checkout can do, in groups. Rows in a radio group (the
// commit mode) select rather than run — the mark on the current pick is what
// says so. Every other row runs on click; a destructive one asks for a second
// click, the first swapping in its question. The menu opens upward, since the
// button is pinned to the panel footer, and walks with the arrow keys: focus
// lands on the first row when it opens, Escape hands it back to the caret.
export function SplitAction({
  primary,
  groups,
  tone,
  mainDisabled,
  mainReason,
  busyLabel,
  onPick,
  onRun,
}: {
  primary: FooterAction;
  groups: FooterGroup[];
  tone: ActionTone;
  mainDisabled: boolean;
  /** Why the main click is dead, when that is worth saying. A plain `title`
   *  rather than the app's `data-tip`: a disabled button gets no pointer events
   *  in the WebView, so the hover-driven bubble would never fire. The same
   *  sentence is printed in the bar, which is what the user actually reads. */
  mainReason?: string | null;
  busyLabel: string | null;
  /** A menu row: a radio row's key to make it the default, any other's to run. */
  onPick: (key: string) => void;
  onRun: () => void;
}) {
  const [open, setOpen] = useState(false);
  // The destructive row waiting for its second click, or null.
  const [armed, setArmed] = useState<string | null>(null);
  const caretRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const busy = busyLabel != null;
  const hasMenu = groups.length > 0;
  const close = (refocus = false) => {
    setOpen(false);
    setArmed(null);
    if (refocus) caretRef.current?.focus();
  };

  // The rows a keyboard can reach, in menu order — a gated row included, so
  // its reason can be reached and read, not only hovered.
  const rows = () =>
    Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button.dd-item") ?? []);
  useEffect(() => {
    if (open) rows()[0]?.focus();
  }, [open]);
  // Work starting under an open menu — an action of ours, or the agent's turn —
  // closes it: a row confirmed into a busy checkout would race that work.
  useEffect(() => {
    if (busy) {
      setOpen(false);
      setArmed(null);
    }
  }, [busy]);
  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const list = rows();
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    const go = (i: number) => list[(i + list.length) % list.length]?.focus();
    switch (e.key) {
      case "ArrowDown":
        go(at + 1);
        break;
      case "ArrowUp":
        go(at - 1);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(list.length - 1);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const toneClass = tone !== "accent" ? tone : "";
  // Work in flight shows on the main button only while the gate holds the
  // primary for it; a primary the gate lets through (viewing the PR, focusing
  // it) stays itself, and the menu stays reachable — its rows carry their own
  // gates — so the status slot narrates the work without locking the panel.
  const showBusy = busy && mainDisabled;

  return (
    <div
      className={`git-split ${toneClass} ${showBusy ? "busy" : ""}`}
      // Escape closes the menu from anywhere in the control, the caret included.
      onKeyDown={(e) => {
        if (open && e.key === "Escape") {
          e.preventDefault();
          close(true);
        }
      }}
    >
      <button
        type="button"
        className="gsa-main text-base"
        disabled={mainDisabled}
        title={mainReason ?? undefined}
        onClick={onRun}
      >
        {showBusy ? <Spinner /> : <Icon name={primary.icon} />}
        <span className="gsa-label">{showBusy ? busyLabel : primary.label}</span>
      </button>
      {hasMenu && (
        <button
          ref={caretRef}
          type="button"
          className="gsa-caret tip"
          data-tip="More actions"
          aria-label="More actions"
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={() => (open ? close() : setOpen(true))}
        >
          <Icon name="chevU" />
        </button>
      )}
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 199 }} onClick={() => close()} />
          <DropdownMenu
            ref={menuRef}
            className="gsa-menu"
            role="menu"
            onKeyDown={onMenuKey}
            // Tabbing out closes it, like a click outside would.
            onBlur={(e) => {
              const to = e.relatedTarget as Node | null;
              if (to && (menuRef.current?.contains(to) || caretRef.current === to)) return;
              close();
            }}
          >
            {groups.map((g, gi) => (
              <div key={g.title ?? `group-${gi}`} role={g.radio ? "group" : undefined}>
                {gi > 0 && <DropdownSeparator />}
                {g.title && <DropdownSection>{g.title}</DropdownSection>}
                {g.items.map((a) => {
                  const isArmed = armed === a.key;
                  return (
                    // A gated row stays focusable (`aria-disabled`, not the
                    // native attribute) and says why beneath its label, so the
                    // reason reaches keyboard and screen-reader users too.
                    <DropdownItem
                      key={a.key}
                      as="button"
                      role={g.radio ? "menuitemradio" : "menuitem"}
                      aria-checked={g.radio ? a.active : undefined}
                      aria-disabled={a.disabled || undefined}
                      active={a.active}
                      danger={a.danger || isArmed}
                      className={[g.radio ? "gsa-radio" : "", a.disabled ? "is-disabled" : ""]
                        .filter(Boolean)
                        .join(" ")}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => {
                        if (a.disabled) return;
                        if (a.confirm && !isArmed) {
                          setArmed(a.key);
                          return;
                        }
                        onPick(a.key);
                        close(true);
                      }}
                    >
                      <div className="di-i">
                        <Icon name={a.icon} size={12} />
                      </div>
                      <span className="di-l">
                        {isArmed ? a.confirm : a.label}
                        {a.disabled && a.reason && <span className="gsa-why">{a.reason}</span>}
                      </span>
                      {g.radio && a.active && (
                        <span className="di-m gsa-default" aria-hidden>
                          default
                        </span>
                      )}
                    </DropdownItem>
                  );
                })}
              </div>
            ))}
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
