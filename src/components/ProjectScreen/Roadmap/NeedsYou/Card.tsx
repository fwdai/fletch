// NeedsYou/Card.tsx — one decision card. Every card names its item and offers
// exactly one gesture, and the gesture is the existing one for that decision:
// a question is answered inline, an approval opens the review surface Mission
// Control opens, a conflict or a spent budget goes to the run (v1 resolves
// neither from here — both need the run's own state to decide), and a wedge
// focuses the item whose dependencies have to change.

import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { pausedLabel } from "@/workflows/run/status";
import { AnswerCard } from "./AnswerCard";
import type { NeedsCard, NeedsReason } from "./select";

/** The glyph per reason. Not a label: a run pause is named by `pausedLabel`, the
 *  same words the item card's chip and the sidebar badge use, so the three can't
 *  drift apart. */
const GLYPH: Record<NeedsReason, IconName> = {
  "workflow-question": "feedback",
  "workflow-approval": "diff",
  "workflow-conflict": "merge",
  "workflow-budget": "zap",
  "item-blocked": "graph",
};

/** What the card says it is waiting on. A block spells out the cycle the event
 *  recorded ("MCA-101 → MCA-104 → MCA-101"), which is the whole point of the
 *  durable event: the transient queue note said only "Waiting on…". */
function whyLine(card: NeedsCard): string {
  if (card.pausedReason) return pausedLabel(card.pausedReason);
  return card.detail ? `dependency cycle — ${card.detail}` : "blocked on its dependencies";
}

export function DecisionCard({
  card,
  onFocusItem,
  onOpenRun,
  onReview,
}: {
  card: NeedsCard;
  /** Jump the board to this card's item (expand, scroll, ring). */
  onFocusItem: () => void;
  /** Open the run behind the card — the conflict/budget escape hatch. */
  onOpenRun: () => void;
  /** Mount the shared review surface over the run's approval gate. */
  onReview: () => void;
}) {
  return (
    <div className="rm-needs-card">
      <div className="rm-needs-row flex-center">
        <Icon name={GLYPH[card.reason]} size={11} className="rm-needs-glyph" />
        <button
          type="button"
          className="rm-needs-item iflex-center truncate"
          onClick={onFocusItem}
          title="Show this item on the board"
        >
          <span className="rm-code mono text-xs">{card.code}</span>
          <span className="rm-needs-title truncate">{card.title}</span>
        </button>
        <span className="rm-needs-why truncate">{whyLine(card)}</span>
        <span className="grow" />
        {card.reason === "workflow-approval" && (
          <Button variant="primary" size="sm" onClick={onReview}>
            <Icon name="diff" size={11} /> Review…
          </Button>
        )}
        {(card.reason === "workflow-conflict" || card.reason === "workflow-budget") && (
          <Button variant="ghost" size="sm" onClick={onOpenRun}>
            View run
          </Button>
        )}
      </div>
      {/* The one reason with no modal and no detour: the answer goes here. */}
      {card.reason === "workflow-question" && card.runId && <AnswerCard runId={card.runId} />}
    </div>
  );
}
