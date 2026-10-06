import { type KeyboardEvent, useRef } from "react";
import type { PrSetEntry } from "@/api";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { splitChips } from "./chips";
import { PrChip } from "./PrChip";
import { PrOverflowMenu } from "./PrOverflowMenu";

/** The checkout's PRs as a row of tabs under the status header, one of them
 *  focused — the PR the header, card, checks, threads and action bar below all
 *  describe. Picking another focuses it on the host (`focusPr`); the panel
 *  follows through the store's focused maps, with no wiring of its own.
 *
 *  Rendered by `GitRepoSection` only once the checkout holds two or more PRs,
 *  so a one-PR panel is exactly what it was. Each repo of a multi-repo panel has
 *  its own, keyed by its checkout. */
export function PrSwitcher({
  agentId,
  subdir,
  entries,
  focused,
}: {
  agentId: string;
  subdir?: string;
  /** The checkout's PR set (`prSets`), newest number first. */
  entries: PrSetEntry[];
  /** The focused PR's number (`prStates`), or null while none is known. */
  focused: number | null;
}) {
  const focusPr = useAppStore((s) => s.focusPr);
  const gate = useGate("focusPr");
  const chipRefs = useRef(new Map<number, HTMLButtonElement>());

  const { inline, overflow } = splitChips(entries, focused);
  const select = (number: number) => void focusPr(agentId, number, subdir);
  // Roving tabindex: one chip is in the tab order — the selected one, or the
  // first while the focused PR is unknown, so the row is never unreachable.
  const tabStop = inline.some((e) => e.state.number === focused)
    ? focused
    : (inline[0]?.state.number ?? null);

  // WAI-ARIA tabs with automatic activation: the arrows move focus along the
  // chips (wrapping) and select what they land on.
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, at: number) => {
    if (gate || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
    e.preventDefault();
    const step = e.key === "ArrowRight" ? 1 : -1;
    const next = inline[(at + step + inline.length) % inline.length];
    chipRefs.current.get(next.state.number)?.focus();
    if (next.state.number !== focused) select(next.state.number);
  };

  return (
    <div className="git-pr-set git-pr-switch">
      <div className="git-pr-switch-tabs flex-center" role="tablist" aria-label="Pull requests">
        {inline.map((entry, i) => {
          const number = entry.state.number;
          return (
            <PrChip
              key={number}
              entry={entry}
              selected={number === focused}
              tabbable={number === tabStop}
              disabledReason={gate}
              buttonRef={(el) => {
                if (el) chipRefs.current.set(number, el);
                else chipRefs.current.delete(number);
              }}
              onSelect={() => select(number)}
              onKeyDown={(e) => onKeyDown(e, i)}
            />
          );
        })}
      </div>
      {overflow.length > 0 && (
        <PrOverflowMenu
          entries={entries}
          hidden={overflow.length}
          focused={focused}
          disabledReason={gate}
          onSelect={select}
        />
      )}
    </div>
  );
}
