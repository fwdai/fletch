// NeedsYou/Card.tsx — one decision card. Every card names its item and offers
// exactly one gesture, and the gesture is the existing one for that decision: a
// wedge focuses the item whose dependencies have to change, and a hold is
// released outright — the one decision whose whole resolution is a single
// click, because the reason for it is already on the row.

import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NeedsCard, NeedsReason } from "./select";

const GLYPH: Record<NeedsReason, IconName> = {
  "item-blocked": "graph",
  "item-held": "pause",
  "project-held": "pause",
};

/** What the card says it is waiting on. A block spells out the cycle the event
 *  recorded ("MCA-101 → MCA-104 → MCA-101"), which is the whole point of the
 *  durable event: the transient queue note said only "Waiting on…". A hold says
 *  its reason verbatim — it was written for exactly this line. */
function whyLine(card: NeedsCard): string {
  if (card.reason === "item-held") return `held — ${card.detail}`;
  if (card.reason === "project-held") return `the whole board is held — ${card.detail}`;
  return card.detail ? `dependency cycle — ${card.detail}` : "blocked on its dependencies";
}

export function DecisionCard({
  card,
  onFocusItem,
  onRelease,
}: {
  card: NeedsCard;
  /** Jump the board to this card's item (expand, scroll, ring). */
  onFocusItem: () => void;
  /** Lift this card's hold — the item's or the board's, decided by the caller
   *  from the card's reason. Only the user can do this, which is why it is a
   *  button here and not an op the PM has. */
  onRelease: () => void;
}) {
  const held = card.reason === "item-held" || card.reason === "project-held";
  return (
    <div className="rm-needs-card">
      <div className="rm-needs-row flex-center">
        <Icon name={GLYPH[card.reason]} size={11} className="rm-needs-glyph" />
        {/* The board's own hold names no item, so there is nothing to jump to:
            a plain label rather than a button that would ring nothing. */}
        {card.code ? (
          <button
            type="button"
            className="rm-needs-item iflex-center truncate"
            onClick={onFocusItem}
            title="Show this item on the board"
          >
            <span className="rm-code mono text-xs">{card.code}</span>
            <span className="rm-needs-title truncate">{card.title}</span>
          </button>
        ) : (
          <span className="rm-needs-item iflex-center truncate">
            <span className="rm-needs-title truncate">This board</span>
          </span>
        )}
        <span className="rm-needs-why truncate">{whyLine(card)}</span>
        <span className="grow" />
        {held && (
          <Button variant="primary" size="sm" onClick={onRelease}>
            <Icon name="play" size={11} /> Release
          </Button>
        )}
      </div>
    </div>
  );
}
