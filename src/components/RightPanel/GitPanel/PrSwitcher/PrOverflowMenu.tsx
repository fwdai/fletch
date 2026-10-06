import { useRef, useState } from "react";
import type { PrSetEntry } from "@/api";
import { DropdownItem, DropdownMenu } from "@/components/ui/Dropdown";
import { usePlacement } from "@/components/ui/usePlacement";
import { chipStatus } from "../PrSetStrip";
import { prSummary } from "./PrChip";

/** The `+N` button after the chips, and the menu it opens: every PR of the
 *  checkout (not just the hidden ones, so the list reads as the whole set),
 *  with enough to tell them apart — number, title, state and branch. Picking
 *  one focuses it. Built on the same anchored-dropdown pieces as `MenuButton`,
 *  whose trigger is an icon where this one is a count. */
export function PrOverflowMenu({
  entries,
  hidden,
  focused,
  disabledReason,
  onSelect,
}: {
  entries: PrSetEntry[];
  /** How many PRs have no chip — the `+N`. */
  hidden: number;
  focused: number | null;
  disabledReason: string | null;
  onSelect: (number: number) => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const placement = usePlacement(open, wrapRef, menuRef);
  const close = () => setOpen(false);

  return (
    <div className="dd-anchor" ref={wrapRef}>
      <button
        type="button"
        className="git-pr-switch-more text-xs"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`${hidden} more PRs`}
        onClick={() => setOpen((v) => !v)}
      >
        +{hidden}
      </button>
      {open && (
        <>
          {/* Full-viewport scrim: any outside click dismisses the menu. */}
          <div className="dd-anchor-scrim" onClick={close} />
          <DropdownMenu ref={menuRef} role="menu" className={`git-pr-menu ${placement}`}>
            {entries.map((entry) => {
              const { state } = entry;
              const { variant, word } = chipStatus(state, entry.checks);
              const selected = state.number === focused;
              return (
                <DropdownItem
                  key={state.number}
                  as="button"
                  role="menuitemradio"
                  aria-checked={selected}
                  aria-label={prSummary(entry)}
                  title={disabledReason ?? undefined}
                  active={selected}
                  disabled={disabledReason != null && !selected}
                  onClick={() => {
                    close();
                    if (!selected) onSelect(state.number);
                  }}
                >
                  {/* The chip's tint, borrowed from the badge variant so the
                      menu and the chips cannot disagree. */}
                  <span className="di-i">
                    <span className={`ag-badge ${variant} git-pr-dot`} />
                  </span>
                  <span className="di-l">
                    <span className="git-pr-menu-num">#{state.number}</span> {state.title}
                  </span>
                  <span className="di-m">{[word, state.branch].filter(Boolean).join(" · ")}</span>
                </DropdownItem>
              );
            })}
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
