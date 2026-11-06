// Board/NotDoing.tsx — the decision log: items ruled off the board, kept with
// their reason instead of deleted.
//
// A collapsed disclosure under the horizon groups, deliberately quiet — this is
// an archive of decisions, not a fourth column, so it opens on demand and its
// rows carry no chips, no drag, no expandable body. What a row owes the reader
// is exactly the ruling: the code (still linkable from the PM chat), the title,
// the close_reason — and the one gesture that reverses it.

import { useState } from "react";
import type { RoadmapItem } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";

export function NotDoing({
  items,
  focusCode,
  onReopen,
  rowRef,
}: {
  /** Rejected rows, newest ruling first (see partition.ts `rejectedRows`). */
  items: readonly RoadmapItem[];
  /** The board's jumped-to code, so a reveal that lands here is visible. */
  focusCode?: string | null;
  /** Put an item back on the board (`rejected → open`). Absent on a read-only
   *  board. */
  onReopen?: (id: string) => void;
  /** Registers each row for the board's scroll-into-view, same as the cards. */
  rowRef?: (code: string, el: HTMLElement | null) => void;
}) {
  const [open, setOpen] = useState(false);
  // A reveal can name a rejected row (revealTarget calls every non-shipped row
  // "board"), and a closed section would swallow the jump. Derived, not
  // latched: the rows must be mounted in the same render the board's scroll
  // effect goes looking for them, so the focus forces `shown` directly — no
  // effect (too late) and no render-phase setState.
  const focused = focusCode != null && items.some((i) => i.code === focusCode);
  const shown = open || focused;

  if (items.length === 0) return null;
  return (
    <section className="rm-nd">
      <button
        type="button"
        className="rm-nd-h flex-center text-xs"
        onClick={() => setOpen(!shown)}
        aria-expanded={shown}
      >
        <Icon name="archive" size={11} />
        <span className="rm-nd-l mono">Not doing</span>
        <span className="rm-nd-c mono">{items.length}</span>
        <Icon name="chevD" size={9} className="rm-nd-chev" />
      </button>
      {shown && (
        <ul className="rm-nd-list">
          {items.map((i) => (
            <li
              key={i.code}
              className={`rm-nd-row ${focusCode === i.code ? "focus" : ""}`}
              ref={(el) => rowRef?.(i.code, el)}
            >
              <div className="rm-nd-line flex-center">
                <span className="rm-code mono text-xs">{i.code}</span>
                <span className="rm-nd-t text-sm truncate">{i.title}</span>
                <span className="grow" />
                {onReopen && (
                  <Button
                    variant="ghost"
                    size="sm"
                    tip="Put it back on the board as open"
                    onClick={() => onReopen(i.id)}
                  >
                    <Icon name="archiveRestore" size={11} /> Reopen
                  </Button>
                )}
              </div>
              {/* Non-null exactly while the row is rejected — the reason is the
                  log's whole value, so it gets its own line rather than a
                  truncating tail. */}
              {i.close_reason && <div className="rm-nd-why text-xs">{i.close_reason}</div>}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
