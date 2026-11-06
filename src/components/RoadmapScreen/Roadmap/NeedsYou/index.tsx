// NeedsYou — the board's decision strip: every open decision this project's
// pipeline is waiting on the user for, above the horizons, resolved without
// leaving the board.
//
// The derivation is a pure selector (select.ts); this file is the strip shell.
//
// Nothing here removes a card: every action moves backend state (a release lifts
// the hold, an edited dependency unwedges the item), the change arrives on
// `roadmap:item`, and the selector simply stops producing the card. An empty
// strip renders nothing at all — a "nothing needs you" placeholder above every
// board would be noise on the board's resting state.

import { Icon } from "@/components/Icon";
import { DecisionCard } from "./Card";
import type { NeedsCard } from "./select";

export function NeedsYou({
  cards,
  onFocusItem,
  onReleaseItem,
  onReleaseProject,
}: {
  cards: readonly NeedsCard[];
  /** Jump the board to an item by code — the hook's `focusItem`. */
  onFocusItem: (code: string) => void;
  /** Lift one item's hold. Absent on a read-only board, where the strip is a
   *  report rather than a set of levers. */
  onReleaseItem?: (itemId: string) => void;
  /** Lift the board's hold. Absent for the same reason. */
  onReleaseProject?: () => void;
}) {
  if (cards.length === 0) return null;

  return (
    <div className="rm-needs">
      <div className="rm-needs-h flex-center text-xs">
        <span className="rm-needs-n iflex-center mono">
          <Icon name="hand" size={11} />
          Needs you
        </span>
        <span className="rm-needs-hint truncate">
          {cards.length === 1 ? "One decision" : `${cards.length} decisions`} the pipeline is
          waiting on.
        </span>
      </div>
      {cards.map((card) => (
        <DecisionCard
          key={card.id}
          card={card}
          onFocusItem={() => card.code && onFocusItem(card.code)}
          // One prop for both scopes: which release this is follows from the
          // card's reason, and the card should not have to know two callbacks.
          onRelease={() => {
            if (card.reason === "project-held") onReleaseProject?.();
            else if (card.itemId) onReleaseItem?.(card.itemId);
          }}
        />
      ))}
    </div>
  );
}
