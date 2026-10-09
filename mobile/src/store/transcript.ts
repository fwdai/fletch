// Records → chat items, through the desktop's own per-provider adapters.
//
// The desktop's `@/helpers/transcript` does the same thing, but its module also
// holds `applyEvent`, which is typed against the desktop `AppState` — importing
// it would pull the whole desktop store (and its component graph) into mobile's
// typecheck. The adapters themselves are what matter, and those are shared
// verbatim.

import type { SessionRecord, UserTurn } from "@desktop/api/types/session";
import { mergeUserTurns, turnsOnPage } from "@desktop/helpers/mergeTurns";
import { hasSentTurn } from "@desktop/helpers/mirrorTurn";
import { type ChatItem, getAdapter, type RawEvent } from "../adapters";

/** The part of an agent's history the phone holds: every page loaded so far,
 *  concatenated oldest first, and the cursor for the page before them (null
 *  once there is none, or on a host that served the history whole). Kept as
 *  records rather than items because a page can start mid-turn: a tool result
 *  whose call is on the page before only pairs with it once both are reduced
 *  together. */
export interface LoadedHistory {
  records: SessionRecord[];
  older: string | null;
}

/** How the last read of an agent's history went. `ready` with nothing to show
 *  is not an empty chat — a chat is created by its first message — so the
 *  screens treat it as a history the host could not produce. */
export type LogLoad =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; error: string };

/** Render canonical session records exactly as on-disk replay does:
 *  `normalizeTranscript` → `reduce`, each item carrying the `recordSeq` and
 *  `recordSession` of the record it came from. Adapter throws degrade to a
 *  partial log rather than an empty screen. */
export function reduceRecords(provider: string | undefined, records: SessionRecord[]): ChatItem[] {
  const adapter = getAdapter(provider);
  let raw: RawEvent[];
  try {
    raw = adapter.normalizeTranscript(
      records.map((r) => r.body),
      records.map((r) => r.seq),
      records.map((r) => r.session_id),
    );
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

/** The log a page of history draws: its records, with the agent's turns merged
 *  in by position (`mergeUserTurns`) — those the page can place, so a turn on
 *  an older page doesn't land at the top of this one. */
export function renderHistory(
  provider: string | undefined,
  records: SessionRecord[],
  turns: UserTurn[],
): ChatItem[] {
  return mergeUserTurns(reduceRecords(provider, records), turnsOnPage(turns, records));
}

/** What a log holds past the history it was rendered from: the replayed
 *  running turn, live frames, a send not yet in the records. Every item
 *  rendered from a record carries its seq and nothing after the history does,
 *  so it is whatever follows the last stamped item — less the turn bubbles a
 *  re-render of the history (`fresh`) draws itself. */
export function pastHistory(log: ChatItem[], fresh: ChatItem[]): ChatItem[] {
  let end = log.length;
  while (end > 0 && log[end - 1].recordSeq === undefined) end -= 1;
  return log.slice(end).filter((it) => {
    const turnId =
      it.kind === "user_message" || it.kind === "queued_message" ? it.turnId : undefined;
    return turnId === undefined || !hasSentTurn(fresh, turnId);
  });
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
