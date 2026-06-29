import type { TurnTiming } from "../../../adapters";
import type { ViewItem } from "../messages/pair";

export interface TurnFooterPlan {
  /** Item index → completed turn's final duration (seconds). The static
   *  "Ran …" footer renders after that item, doubling as the turn seam. */
  completedFooters: Map<number, number>;
  /** The in-flight turn's timing, driving the live ticker (undefined when no
   *  turn is open). */
  openTurnTiming: TurnTiming | undefined;
}

/** Plan where each completed turn's static footer renders and which turn is
 *  live. A turn spans a user_message to just before the next one; its timing
 *  rides on the opening user_message. A carried-forward `queued_message`
 *  (a follow-up belonging to a *future* turn) is skipped when closing a turn,
 *  so a completed turn's footer renders after its own last item, not after the
 *  queued bubble. */
export function planTurnFooters(items: ViewItem[]): TurnFooterPlan {
  const completedFooters = new Map<number, number>();
  let turnTiming: TurnTiming | undefined;
  let turnOpen = false;
  let openTurnTiming: TurnTiming | undefined;
  // Walk back past trailing queued_message bubbles (follow-ups belonging to the
  // next turn) so the footer lands on the closing turn's own last item.
  const lastContentIdx = (boundary: number) => {
    let j = boundary;
    while (j >= 0 && items[j].kind === "queued_message") j -= 1;
    return j;
  };
  const closeAt = (boundary: number) => {
    if (turnOpen && turnTiming?.completedAt != null) {
      completedFooters.set(lastContentIdx(boundary), turnTiming.activeMs / 1000);
    }
  };
  items.forEach((it, i) => {
    if (it.kind === "user_message") {
      closeAt(i - 1); // the previous turn ends just before this one
      turnTiming = it.timing;
      turnOpen = true;
    }
  });
  if (turnOpen) {
    closeAt(items.length - 1); // close the last turn at the end of the feed
    if (turnTiming && turnTiming.completedAt == null) openTurnTiming = turnTiming;
  }
  return { completedFooters, openTurnTiming };
}
