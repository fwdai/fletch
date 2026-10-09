// Merging Fletch's own record of each prompt (`session_user_turns`) into a log
// reduced from session records. The backend places every turn among its
// session's records (`UserTurn.position`), so the merge is by position and
// state alone: no text matching, no alignment. Pure and free of store types so
// the phone imports it as-is.

import type { ChatItem } from "@/adapters/types";
import type { SessionRecord, UserTurn } from "@/api/types/session";
import { hasSentTurn, sentTurnBubble } from "./mirrorTurn";

type UserMessage = Extract<ChatItem, { kind: "user_message" }>;

/** What a bubble the agent never read says about it, on every client. */
export const UNDELIVERED_LABEL: Record<NonNullable<UserMessage["undelivered"]>, string> = {
  interrupted: "Stopped before the agent read it",
  failed: "Not delivered",
};

/** A record's name in a stitched history: seqs repeat across sessions. A host
 *  that predates sessions on records and turns names neither, and its turns
 *  have no position to look up. */
function recordKey(session: string | undefined, seq: number): string {
  return `${session ?? ""}\u0000${seq}`;
}

/** Overlay a turn's Fletch-origin metadata (id, run timing, attachments) onto
 *  the bubble it belongs to. With attachments, the bubble shows the text the
 *  user typed rather than the transcript's copy, which the runner padded with
 *  `Attached file: <path>` lines; prefix-guarded so a row can never rewrite a
 *  message that isn't its own. */
function overlayTurn(item: UserMessage, t: UserTurn): UserMessage {
  const next: UserMessage = { ...item, turnId: t.turn_id };
  if (t.started_at != null) next.startedAt = t.started_at;
  if (t.ended_at != null) next.endedAt = t.ended_at;
  if (t.attachments.length > 0) {
    next.attachments = t.attachments;
    if (item.text.startsWith(t.text)) next.text = t.text;
  }
  return next;
}

/** A bubble drawn from the row alone, for a turn the log has no prompt for.
 *  A turn the agent never read (no echo, and stopped or dropped) says so. */
function turnBubble(t: UserTurn): UserMessage {
  const item = overlayTurn({ kind: "user_message", text: t.text }, t);
  if (!t.native_id && (t.outcome === "interrupted" || t.outcome === "failed")) {
    item.undelivered = t.outcome;
  }
  return item;
}

/** The bubble for a turn with no position, or null when it needs none. Only a
 *  turn not yet ended qualifies: one running (or awaiting its run), a live
 *  follow-up awaiting its echo, or one dropped before it ran. Unless the log
 *  already draws it (the optimistic or mirrored bubble), it shows at the end,
 *  which is also what keeps a failed send on screen across reloads. An ended
 *  turn without a position is a row from before positions existed: its prompt
 *  is in the transcript already, wherever that put it. */
function unplacedBubble(items: ChatItem[], t: UserTurn): ChatItem | null {
  if (t.ended_at != null || hasSentTurn(items, t.turn_id)) return null;
  if (t.started_at != null || t.outcome != null) return turnBubble(t);
  return sentTurnBubble({ ...t, follow_up: true });
}

/** Merge `turns` into `items`, a log reduced from session records (each item
 *  stamped with its `recordSeq` / `recordSession`), and return the merged log.
 *
 *  - A turn paired with its prompt record (`native_id`) overlays its metadata
 *    onto the `user_message` reduced from exactly that record.
 *  - A turn paired with a record the adapter drew as a notice (a slash
 *    command) adds nothing: the prompt is already on screen.
 *  - A turn with a position but no such bubble — never echoed (`native_id`
 *    null), or echoed in a record the adapter does not draw at all — is drawn
 *    from its row, just before the first item of its session at or past its
 *    position, or after that session's last item. One stopped or dropped
 *    before the agent read it is marked `undelivered`.
 *  - A turn with no position goes at the end, unless the log draws it already
 *    (see `unplacedBubble`).
 *
 *  `turns` come root session first, each session's in send order, as
 *  `read_user_turns` returns them; turns sharing a slot keep that order. */
export function mergeUserTurns(items: ChatItem[], turns: UserTurn[]): ChatItem[] {
  if (turns.length === 0) return items;
  const result = items.slice();
  const prompts = new Map<string, number>();
  // Records the log draws as a notice rather than a bubble (a slash command).
  const noticed = new Set<string>();
  const bySession = new Map<string | undefined, number[]>();
  result.forEach((it, i) => {
    if (it.recordSeq === undefined) return;
    const idxs = bySession.get(it.recordSession) ?? [];
    idxs.push(i);
    bySession.set(it.recordSession, idxs);
    const key = recordKey(it.recordSession, it.recordSeq);
    if (it.kind === "notice") noticed.add(key);
    if (it.kind === "user_message" && !prompts.has(key)) prompts.set(key, i);
  });

  // Bubbles drawn ahead of `result[i]`; `result.length` is the end.
  const before = new Map<number, ChatItem[]>();
  const insert = (at: number, item: ChatItem) => {
    const bucket = before.get(at) ?? [];
    bucket.push(item);
    before.set(at, bucket);
  };
  const slotOf = (session: string | undefined, position: number): number => {
    const idxs = bySession.get(session);
    if (!idxs) return result.length;
    const hit = idxs.find((i) => (result[i].recordSeq ?? 0) >= position);
    return hit ?? idxs[idxs.length - 1] + 1;
  };

  const claimed = new Set<number>();
  const tail: ChatItem[] = [];
  for (const t of turns) {
    if (t.position == null) {
      const bubble = unplacedBubble(items, t);
      if (bubble) tail.push(bubble);
      continue;
    }
    const key = recordKey(t.session_id, t.position);
    const at = t.native_id ? prompts.get(key) : undefined;
    const prompt = at === undefined ? undefined : result[at];
    if (at !== undefined && !claimed.has(at) && prompt?.kind === "user_message") {
      claimed.add(at);
      result[at] = overlayTurn(prompt, t);
    } else if (t.native_id && noticed.has(key)) {
      // The adapter drew the paired record as a notice — a slash command — so
      // the prompt is on screen in the form the adapter chose, and a bubble
      // beside it would show it twice. The row's metadata has nothing to hang
      // on there; a turn like that is not a fork anchor the UI offers anyway.
    } else {
      insert(slotOf(t.session_id, t.position), turnBubble(t));
    }
  }

  if (before.size === 0) return tail.length === 0 ? result : [...result, ...tail];
  const merged: ChatItem[] = [];
  result.forEach((it, i) => {
    const add = before.get(i);
    if (add) merged.push(...add);
    merged.push(it);
  });
  merged.push(...(before.get(result.length) ?? []), ...tail);
  return merged;
}

/** The turns one page of history can place: `records` is the page, possibly
 *  starting mid-session with older pages unread. A positioned turn below the
 *  page's first record of its session is on an older page; one from an
 *  ancestor session the page doesn't reach has nowhere to go. The agent's own
 *  session is always the newest, so a page without any of its records means
 *  it has none yet, and its turns are all newer than the page. */
export function turnsOnPage(turns: UserTurn[], records: SessionRecord[]): UserTurn[] {
  const first = new Map<string | undefined, number>();
  for (const r of records) if (!first.has(r.session_id)) first.set(r.session_id, r.seq);
  return turns.filter((t) => {
    if (t.position == null) return true;
    const from = first.get(t.session_id);
    if (from === undefined) return !t.inherited;
    return t.position >= from;
  });
}
