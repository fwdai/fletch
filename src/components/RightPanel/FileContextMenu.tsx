// A small right-click context menu, positioned at the cursor and clamped to
// the viewport. Generic on its items so it isn't tied to the file tree — pass
// a flat list of entries (with "sep" separators) and it renders + dismisses
// itself on outside-click, Esc, scroll, or resize.
//
// Items may opt into a two-click confirm (`confirmLabel`): the first click
// arms the item (swapping in the confirm label + danger styling), the second
// runs it. Used to guard destructive actions like deleting a folder.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSeparator } from "@/components/ui/Dropdown";
import { useDismiss } from "@/util/hooks";

export interface ContextMenuItem {
  icon: IconName;
  label: string;
  onClick: () => void;
  danger?: boolean;
  /** When set, the item requires a second click; its label becomes this. */
  confirmLabel?: string;
  /** When set, clicking runs the action then briefly shows this label with a
   *  check before the menu dismisses — used to confirm fire-and-forget actions
   *  like "Copy Path". */
  feedbackLabel?: string;
}

export type ContextMenuEntry = ContextMenuItem | "sep";

interface Props {
  x: number;
  y: number;
  entries: ContextMenuEntry[];
  onClose: () => void;
}

export function FileContextMenu({ x, y, entries, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });
  // Index of the item currently armed for confirm, or -1 when none.
  const [armed, setArmed] = useState(-1);
  // Index of the item showing post-click feedback, or -1 when none.
  const [done, setDone] = useState(-1);
  // Pending deferred-close timer (feedbackLabel path), cleared on unmount so it
  // can't fire onClose again after an early close (Esc / outside-click / blur).
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (closeTimer.current) clearTimeout(closeTimer.current);
    },
    [],
  );

  // Clamp into the viewport once we know the menu's size, so a click near the
  // right/bottom edge doesn't push it off-screen.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { offsetWidth: w, offsetHeight: h } = el;
    const nx = Math.min(x, window.innerWidth - w - 8);
    const ny = Math.min(y, window.innerHeight - h - 8);
    setPos({ x: Math.max(8, nx), y: Math.max(8, ny) });
  }, [x, y]);

  useDismiss(ref, true, onClose);
  // Also bail if the viewport shifts under the (fixed-positioned) menu.
  useEffect(() => {
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  return (
    <DropdownMenu
      ref={ref}
      className="fp-ctx"
      style={{ position: "fixed", left: pos.x, top: pos.y }}
      role="menu"
      onScroll={onClose}
    >
      {entries.map((entry, i) => {
        if (entry === "sep") return <DropdownSeparator key={i} />;
        const isArmed = armed === i;
        const isDone = done === i;
        return (
          <DropdownItem
            key={i}
            as="button"
            role="menuitem"
            className="fp-ctx-item"
            danger={entry.danger || isArmed}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              if (entry.confirmLabel && !isArmed) {
                setArmed(i);
                return;
              }
              entry.onClick();
              if (entry.feedbackLabel) {
                setDone(i);
                closeTimer.current = setTimeout(onClose, 900);
              } else {
                onClose();
              }
            }}
          >
            <span className="di-i">
              <Icon name={isDone ? "check" : entry.icon} size={13} />
            </span>
            <span className="di-l">
              {isArmed ? entry.confirmLabel : isDone ? entry.feedbackLabel : entry.label}
            </span>
          </DropdownItem>
        );
      })}
    </DropdownMenu>
  );
}
