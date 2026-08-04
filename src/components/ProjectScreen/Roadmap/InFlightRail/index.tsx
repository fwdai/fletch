// InFlightRail — the board's pulse: one line per item actually in motion, above
// the horizon groups and beside the "Needs you" strip. `active` rows say which
// run is building them and for how long; `in_review` rows say what their PR is
// waiting on. Level-1 answer to "what is happening right now", which the board
// otherwise only tells you by scrolling three groups looking for pearls.
//
// The two strips are complements, which is why they sit together and look
// different: NeedsYou is warn-toned and every card is a decision you owe it;
// this is neutral and carries no decision at all — every entry's one gesture is
// the jump to its card. Nothing in flight means nothing to say, so an empty rail
// renders nothing rather than a "nothing is running" placeholder above every
// resting board.
//
// The derivation is a pure selector (select.ts). All this file adds is the clock:
// the elapsed span has to re-render on its own, and a selector that read the
// time would be untestable.

import { Icon } from "@/components/Icon";
import { formatAge } from "@/util/format";
import { useMinuteClock } from "@/util/hooks";
import type { RailEntry } from "./select";

/** What the strip is looking at, in the counts' own words. Both halves are named
 *  because they mean different things to the reader: one is spending tokens, the
 *  other is waiting on GitHub. */
function hint(entries: readonly RailEntry[]): string {
  const building = entries.filter((e) => e.kind === "active").length;
  const shipping = entries.length - building;
  const parts: string[] = [];
  if (building) parts.push(`${building} being built`);
  if (shipping) parts.push(`${shipping} waiting to ship`);
  return `${parts.join(", ")}.`;
}

export function InFlightRail({
  entries,
  onFocusItem,
}: {
  entries: readonly RailEntry[];
  /** Jump the board to an item by code — the hook's `focusItem`, the same path
   *  the decision strip and the chat's code chips take. */
  onFocusItem: (code: string) => void;
}) {
  const now = useMinuteClock();

  if (entries.length === 0) return null;

  return (
    <div className="rm-rail">
      <div className="rm-rail-h flex-center text-xs">
        <span className="rm-rail-n iflex-center mono">
          <Icon name="activity" size={11} />
          In flight
        </span>
        <span className="rm-rail-hint truncate">{hint(entries)}</span>
      </div>
      {entries.map((e) => (
        <button
          key={e.id}
          type="button"
          className="rm-rail-e flex-center"
          onClick={() => onFocusItem(e.code)}
          title="Show this item on the board"
        >
          {/* The same pearl the card's live chip uses for a running row, and the
              PR glyph for one in review — the rail's two halves are told apart at
              a glance, before any word is read. */}
          {e.kind === "active" ? (
            <span className="rm-pearl" />
          ) : (
            <Icon name="pr" size={11} className="rm-rail-glyph" />
          )}
          <span className="rm-code mono text-xs">{e.code}</span>
          <span className="rm-rail-title truncate">{e.title}</span>
          <span className={`rm-rail-s tone-${e.tone}`}>{e.state}</span>
          {/* How long its run has been at it. Absent for a row the drainer has
              claimed but whose run hasn't been recorded yet, and for everything
              in review — a PR's age is not this board's clock. */}
          {e.startedAt != null && (
            <span className="rm-rail-age mono" title="Elapsed since its run started">
              {formatAge(e.startedAt, now)}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}
