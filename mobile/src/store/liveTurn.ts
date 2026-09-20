// Rebuilding a busy agent's log. The host's records stop at the last finished
// turn — the running one is ingested only when it ends — so the log is the
// records plus the running turn, replayed from the host's own copy of its
// event stream (`read_live_turn`). That copy exists for every provider, and it
// is what a phone that missed the stream in the background renders from.

import type { AgentManagedEvent } from "@desktop/api/types/agent";
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

/** A live frame the replay already folded: numbered below the watermark the
 *  snapshot set for its agent. Folding it again would draw the event twice —
 *  the host forwards events and answers requests on separate tasks, so a frame
 *  the snapshot holds can still reach the phone after the snapshot does. A
 *  frame with no number is from a host without the op, which never replays. */
export function isReplayed(liveSeq: Record<string, number>, e: AgentManagedEvent): boolean {
  return e.seq !== undefined && e.seq < (liveSeq[e.agent_id] ?? 0);
}

/** Fold the host's copy of the running turn onto `base`, as one state patch.
 *
 *  `late` are the frames that reached this client while the snapshot was in
 *  flight. Those the snapshot already holds (`seq` below its `next_seq`) are in
 *  `live.events` and are not folded again; the rest happened after the host
 *  took the snapshot, so they are folded after it — otherwise the replaced log
 *  would lack them for the rest of the turn. The agent's watermark is then
 *  `next_seq`, which is what lets the live handler drop a frame the snapshot
 *  already covered that arrives after this patch lands.
 *
 *  The agent's pending prompts start over: the host drops an answered prompt
 *  from the turn it hands back, so the replay is the whole set of prompts still
 *  held — anything this client remembered predates the frames it missed. */
export function replayLiveTurn(
  s: MobileState,
  agentId: string,
  base: ChatItem[],
  live: LiveTurn,
  late: AgentManagedEvent[] = [],
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
  const fold = (ev: RawEvent) => {
    next = { ...next, ...foldAgentEvent(next, agentId, ev) };
  };
  for (const ev of live.events) fold(ev as RawEvent);
  for (const e of late) {
    if (e.seq === undefined || e.seq >= live.next_seq) fold(e.event as RawEvent);
  }
  return {
    logs: next.logs,
    busy: next.busy,
    pendingToolUse: next.pendingToolUse,
    backgroundTasks: next.backgroundTasks,
    liveSeq: { ...s.liveSeq, [agentId]: live.next_seq },
  };
}
