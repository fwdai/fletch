// Rebuilding a busy agent's log. The host's records stop at the last finished
// turn — the running one is ingested only when it ends — so the log is the
// records plus the running turn, replayed from the host's own copy of its
// event stream (`read_live_turn`). That copy exists for every provider, and it
// is what a phone that missed the stream in the background renders from.

import type { LiveTurn, UserTurn } from "@desktop/api/types/session";
import { mirrorSentTurn } from "@desktop/helpers/mirrorTurn";
import type { ChatItem, RawEvent } from "../adapters";
import { foldAgentEvent } from "./events";
import type { MobileState } from "./index";

/** The turns the records do not carry yet: the one running (`started_at` set)
 *  and the follow-ups queued behind it, neither ended. Drawn as the bubbles
 *  their `turn:sent` would have drawn, so a replayed turn opens with its
 *  prompt. A turn the host has matched to a record is in `items` already. */
export function withPendingTurns(
  agentId: string,
  items: ChatItem[],
  turns: UserTurn[],
): ChatItem[] {
  let next = items;
  for (const t of turns) {
    if (t.native_id || t.ended_at != null) continue;
    next = mirrorSentTurn(next, {
      agent_id: agentId,
      turn_id: t.turn_id,
      text: t.text,
      attachments: t.attachments,
      follow_up: t.started_at == null,
    });
  }
  return next;
}

/** When the running turn began, for the live timer a missed `turn:started`
 *  would have anchored. */
export function runningTurnStart(turns: UserTurn[]): number | undefined {
  return turns.find((t) => t.started_at != null && t.ended_at == null)?.started_at ?? undefined;
}

/** Fold the host's copy of the running turn onto `base`, as one state patch.
 *  Applied in a single `set` so a live frame arriving meanwhile lands after the
 *  replay, never inside it. The agent's pending prompts start over: the host
 *  drops an answered prompt from the turn it hands back, so the replay is the
 *  whole set of prompts still held — anything this client remembered predates
 *  the frames it missed. */
export function replayLiveTurn(
  s: MobileState,
  agentId: string,
  base: ChatItem[],
  live: LiveTurn,
): Partial<MobileState> {
  const head: ChatItem[] =
    live.dropped > 0
      ? [
          ...base,
          {
            kind: "notice",
            subtype: "info",
            text: `${live.dropped} earlier events of this turn were not kept by the host`,
          },
        ]
      : base;
  let next: MobileState = {
    ...s,
    logs: { ...s.logs, [agentId]: head },
    pendingToolUse: { ...s.pendingToolUse, [agentId]: {} },
  };
  for (const ev of live.events) {
    next = { ...next, ...foldAgentEvent(next, agentId, ev as RawEvent) };
  }
  return {
    logs: next.logs,
    busy: next.busy,
    pendingToolUse: next.pendingToolUse,
    backgroundTasks: next.backgroundTasks,
  };
}
