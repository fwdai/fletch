import type { ChatItem } from "@/adapters/types";
import type { TurnSentEvent } from "@/api/types/session";

/** The bubble a client drew for turn `turnId`. */
function isTurnBubble(it: ChatItem, turnId: string): boolean {
  return (it.kind === "user_message" || it.kind === "queued_message") && it.turnId === turnId;
}

/** The log already carries this turn — the sender's own optimistic bubble. */
export function hasSentTurn(items: ChatItem[], turnId: string): boolean {
  return items.some((it) => isTurnBubble(it, turnId));
}

/** The bubble of a message sent but not yet delivered — waiting for its place
 *  in its agent's line or its folders (`store/sendOrder`). A store-only
 *  `queued_message`, which a transcript rebuild keeps until its turn is drawn
 *  (`carryForwardStoreOnly`); without the badge, which says the host is
 *  holding it, so it reads as the plain bubble it most often becomes. */
export function waitingBubble(text: string, turnId: string, attachments: string[]): ChatItem {
  return attachments.length > 0
    ? { kind: "queued_message", text, attachments, turnId }
    : { kind: "queued_message", text, turnId };
}

/** `items` with turn `turnId`'s bubble replaced in place by `next`, or taken
 *  out for `null`. `next` is appended when the bubble is gone (a reload
 *  cleared the log meanwhile). */
export function replaceTurnBubble(
  items: ChatItem[],
  turnId: string,
  next: ChatItem | null,
): ChatItem[] {
  const at = items.findIndex((it) => isTurnBubble(it, turnId));
  if (at < 0) return next ? [...items, next] : items;
  const out = [...items];
  if (next) out[at] = next;
  else out.splice(at, 1);
  return out;
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
