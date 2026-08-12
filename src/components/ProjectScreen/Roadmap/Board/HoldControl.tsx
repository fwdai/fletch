// Board/HoldControl.tsx — the two halves of a hold on a card: the chip that says
// it is held (and lets the user lift it), and the action that places one.
//
// Their own file rather than more branches inside ItemCard: the chip is the
// ghostbar's grammar (a band between the header and the body, ruled on without an
// expand), and placing a hold needs a *reason* — the inline ask lives in
// ReasonAction.tsx, shared with Reject, so the two reasoned commands stay one
// gesture.

import type { RoadmapEventActor } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { ReasonAction } from "./ReasonAction";

/** Who stopped it, in the words the card uses elsewhere. The distinction matters:
 *  a hold the PM placed is news, one the user placed is a note to self. */
function heldBy(by: RoadmapEventActor | null): string {
  return by === "pm" ? "Held by the PM" : "Held";
}

/** The always-visible band on a held row: why nothing is happening, and the one
 *  gesture that changes it. Warn-toned like the queue note, because the row has
 *  stopped — but with an action, because unlike a queue note this one is only
 *  waiting on the person reading it. */
export function HoldChip({
  reason,
  by,
  onRelease,
}: {
  reason: string;
  by: RoadmapEventActor | null;
  /** Absent on a read-only board. */
  onRelease?: () => void;
}) {
  return (
    <div className="rm-hold flex-center text-xs">
      <Icon name="pause" size={11} />
      <span className="rm-hold-t">
        <strong>{heldBy(by)}</strong> — {reason}
      </span>
      <span className="grow" />
      {onRelease && (
        <Button variant="ghost" size="sm" onClick={onRelease}>
          <Icon name="play" size={11} /> Release
        </Button>
      )}
    </div>
  );
}

/** "Hold" as a secondary action, with the reason asked for in place
 *  (ReasonAction): a hold with no reason leaves the user a Release button and
 *  no idea what it undoes. */
export function HoldAction({ onHold }: { onHold: (reason: string) => void }) {
  return (
    <ReasonAction
      icon="pause"
      label="Hold"
      tip="Stop the queue from building this until you release it"
      placeholder="What has to be agreed first?"
      onCommit={onHold}
    />
  );
}
