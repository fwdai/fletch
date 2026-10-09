// Fold a `turn:sent` event into a chat log, so a message sent from one device
// shows on every other one while the turn is still running. Pure and free of
// store types so the phone imports it as-is.

import type { ChatItem } from "@/adapters/types";
import type { TurnSentEvent } from "@/api/types/session";

/** The log already carries this turn — the sender's own optimistic bubble. */
export function hasSentTurn(items: ChatItem[], turnId: string): boolean {
  return items.some(
    (it) => (it.kind === "user_message" || it.kind === "queued_message") && it.turnId === turnId,
  );
}

/** The bubble a sent turn draws: a follow-up as the same `queued_message` the
 *  sender drew, a turn-opening message as a `user_message`. */
export function sentTurnBubble(
  e: Pick<TurnSentEvent, "turn_id" | "text" | "attachments" | "follow_up">,
): ChatItem {
  const attachments = e.attachments.length > 0 ? { attachments: e.attachments } : {};
  return e.follow_up
    ? { kind: "queued_message", text: e.text, turnId: e.turn_id, ...attachments }
    : { kind: "user_message", text: e.text, turnId: e.turn_id, ...attachments };
}

/** Append the mirrored bubble for `e`, or return `items` untouched (same
 *  reference) when it is already there. Reconciled away by the turn-end
 *  rebuild exactly like the sender's. */
export function mirrorSentTurn(items: ChatItem[], e: TurnSentEvent): ChatItem[] {
  if (hasSentTurn(items, e.turn_id)) return items;
  return [...items, sentTurnBubble(e)];
}
