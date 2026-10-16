import { useState } from "react";
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
// button is pinned to the panel footer.
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
  const busy = busyLabel != null;
  const hasMenu = groups.length > 0;
  const close = () => {
    setOpen(false);
    setArmed(null);
  };

  const toneClass = tone !== "accent" ? tone : "";

  return (
    <div className={`git-split ${toneClass} ${busy ? "busy" : ""}`}>
      <button
        type="button"
        className="gsa-main text-base"
        disabled={mainDisabled || busy}
        title={mainReason ?? undefined}
        onClick={onRun}
      >
        {busy ? <Spinner /> : <Icon name={primary.icon} />}
        <span className="gsa-label">{busy ? busyLabel : primary.label}</span>
      </button>
      {hasMenu && (
        <button
          type="button"
          className="gsa-caret tip"
          data-tip="More actions"
          aria-label="More actions"
          aria-expanded={open}
          disabled={busy}
          onClick={() => (open ? close() : setOpen(true))}
        >
          <Icon name="chevU" />
        </button>
      )}
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 199 }} onClick={close} />
          <DropdownMenu className="gsa-menu" role="menu">
            {groups.map((g, gi) => (
              <div key={g.title ?? `group-${gi}`} role={g.radio ? "group" : undefined}>
                {gi > 0 && <DropdownSeparator />}
                {g.title && <DropdownSection>{g.title}</DropdownSection>}
                {g.items.map((a) => {
                  const isArmed = armed === a.key;
                  return (
                    <DropdownItem
                      key={a.key}
                      as="button"
                      role={g.radio ? "menuitemradio" : "menuitem"}
                      aria-checked={g.radio ? a.active : undefined}
                      active={a.active}
                      disabled={a.disabled}
                      danger={a.danger || isArmed}
                      className={g.radio ? "gsa-radio" : undefined}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => {
                        if (a.confirm && !isArmed) {
                          setArmed(a.key);
                          return;
                        }
                        onPick(a.key);
                        close();
                      }}
                    >
                      <div className="di-i">
                        <Icon name={a.icon} size={12} />
                      </div>
                      <span className="di-l">{isArmed ? a.confirm : a.label}</span>
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
