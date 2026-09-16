// Records → chat items, through the desktop's own per-provider adapters.
//
// The desktop's `@/helpers/transcript` does the same thing, but its module also
// holds `applyEvent`, which is typed against the desktop `AppState` — importing
// it would pull the whole desktop store (and its component graph) into mobile's
// typecheck. The adapters themselves are what matter, and those are shared
// verbatim.

import type { SessionRecord, UserTurn } from "@desktop/api/types/session";
import { type ChatItem, getAdapter, type RawEvent } from "../adapters";

/** Render canonical session records exactly as on-disk replay does:
 *  `normalizeTranscript` → `reduce`. Adapter throws degrade to a partial log
 *  rather than an empty screen. */
export function reduceRecords(provider: string | undefined, records: SessionRecord[]): ChatItem[] {
  const adapter = getAdapter(provider);
  let raw: RawEvent[];
  try {
    raw = adapter.normalizeTranscript(records.map((r) => r.body));
  } catch {
    return [];
  }
  let items: ChatItem[] = [];
  for (const ev of raw) {
    try {
      items = adapter.reduce(items, ev);
    } catch {
      // Skip the one bad event; keep the rest of the transcript.
    }
  }
  return items;
}

/** Overlay a turn's Fletch-origin metadata — run timing and attachments — from
 *  `session_user_turns` onto the rendered user messages, end-aligned so turns
 *  predating the rows keep none. A turn with attachments also restores the text
 *  the user actually typed: the transcript's copy is what the runner sent,
 *  padded with `Attached file: <path>` lines, so the rebuilt bubble would
 *  otherwise differ from the one the send drew. Prefix-guarded, as on the
 *  desktop, so a mis-aligned row cannot rewrite an unrelated message. */
export function applyUserTurns(items: ChatItem[], turns: UserTurn[]): ChatItem[] {
  if (turns.length === 0) return items;
  const matched = turns.filter((t) => t.native_id);
  const result = items.map((it) => ({ ...it }));
  const userIdxs = result.flatMap((it, i) => (it.kind === "user_message" ? [i] : []));
  const n = Math.min(matched.length, userIdxs.length);
  for (let k = 1; k <= n; k += 1) {
    const turn = matched[matched.length - k];
    const item = result[userIdxs[userIdxs.length - k]];
    if (item.kind !== "user_message") continue;
    if (turn.started_at != null) item.startedAt = turn.started_at;
    if (turn.ended_at != null) item.endedAt = turn.ended_at;
    if (turn.attachments.length > 0) {
      item.attachments = turn.attachments;
      if (item.text.startsWith(turn.text)) item.text = turn.text;
    }
  }
  return result;
}

/** Apply one live event to an agent's log. Returns the next log plus whether
 *  the turn ended (any adapter's `turn_end` notice), mirroring the desktop's
 *  `applyEvent` contract. */
export function applyLiveEvent(
  provider: string | undefined,
  prev: ChatItem[],
  rawEvent: RawEvent,
): { items: ChatItem[]; turnEnded: boolean } {
  const adapter = getAdapter(provider);
  let next: ChatItem[];
  try {
    next = adapter.reduce(prev, rawEvent);
  } catch {
    return { items: prev, turnEnded: false };
  }
  if (next === prev) return { items: prev, turnEnded: false };
  const last = next[next.length - 1];
  const turnEnded =
    next.length > prev.length && last?.kind === "notice" && last.subtype === "turn_end";
  return { items: next, turnEnded };
}
