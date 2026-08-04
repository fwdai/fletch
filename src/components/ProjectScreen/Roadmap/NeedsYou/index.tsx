// NeedsYou — the board's decision strip: every open decision this project's
// pipeline is waiting on the user for, above the horizons. Level-2 injection
// (answer a question, rule on a gate) without leaving level 1.
//
// The derivation is a pure selector (select.ts) in MissionControl/queue.ts's
// style, and shares its reason vocabulary where the two overlap — Mission
// Control is the same question fleet-wide, and the two surfaces must never
// disagree about what a signal means. This file is the strip shell and the host
// for the review modal Mission Control also mounts.
//
// Nothing here removes a card: every action moves backend state (an answered
// question resumes its run, an approval advances it, an edited dependency
// unwedges the item), the change arrives on `wf:run` / `roadmap:item`, and the
// selector simply stops producing the card. An empty strip renders nothing at
// all — a "nothing needs you" placeholder above every board would be noise on
// the board's resting state.

import { useState } from "react";
import { Icon } from "@/components/Icon";
import { WorkflowReviewModal } from "@/components/Workspace/MissionControl/WorkflowReviewModal";
import { DecisionCard } from "./Card";
import type { NeedsCard } from "./select";

export function NeedsYou({
  cards,
  onFocusItem,
  onOpenRun,
  onReleaseItem,
  onReleaseProject,
}: {
  cards: readonly NeedsCard[];
  /** Jump the board to an item by code — the hook's `focusItem`. */
  onFocusItem: (code: string) => void;
  /** Select a run and get out of the way — the card's "View run". */
  onOpenRun: (runId: string) => void;
  /** Lift one item's hold. Absent on a read-only board, where the strip is a
   *  report rather than a set of levers. */
  onReleaseItem?: (itemId: string) => void;
  /** Lift the board's hold. Absent for the same reason. */
  onReleaseProject?: () => void;
}) {
  const [reviewRunId, setReviewRunId] = useState<string | null>(null);

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
          onOpenRun={() => card.runId && onOpenRun(card.runId)}
          onReview={() => card.runId && setReviewRunId(card.runId)}
          // One prop for both scopes: which release this is follows from the
          // card's reason, and the card should not have to know two callbacks.
          onRelease={() => {
            if (card.reason === "project-held") onReleaseProject?.();
            else if (card.itemId) onReleaseItem?.(card.itemId);
          }}
        />
      ))}
      {/* The same modal the review queue mounts: evidence from the run's
          `gate_evidence` event, approve/reject through wfApprove / wfReject. It
          closes itself when the run leaves the gate, however that happened. */}
      {reviewRunId && (
        <WorkflowReviewModal runId={reviewRunId} onClose={() => setReviewRunId(null)} />
      )}
    </div>
  );
}
