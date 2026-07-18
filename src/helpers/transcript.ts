// Transcript / log reduction: rendering canonical session_records into chat
// items, overlaying outgoing user-turn metadata, carrying forward store-only
// items across a rebuild, and applying a single live event.

import { type ChatItem, getAdapter, type RawEvent } from "../adapters";
import type { SessionRecord, UserTurn } from "../api";
import type { AppState } from "../store";
import { providerFor } from "./agentLookups";

/** Render canonical `session_records` (verbatim per-provider transcript
 *  bodies) into chat items via the same pipeline as on-disk replay:
 *  `normalizeTranscript` → `reduce`. Defensive: a malformed body or an adapter
 *  throw degrades gracefully instead of failing the whole restore. */
export function reduceRecords(provider: string | undefined, records: SessionRecord[]): ChatItem[] {
  const adapter = getAdapter(provider);
  let rawEvents: RawEvent[];
  try {
    rawEvents = adapter.normalizeTranscript(records.map((r) => r.body));
  } catch (err) {
    console.error("[adapters] normalizeTranscript threw during restore", {
      provider,
      err,
    });
    return [];
  }
  let items: ChatItem[] = [];
  for (const ev of rawEvents) {
    try {
      items = adapter.reduce(items, ev);
    } catch (err) {
      console.error("[adapters] reduce threw during records restore", {
        provider,
        type: ev.type,
        err,
      });
    }
  }
  return items;
}

/** Overlay Fletch-origin outgoing-turn metadata (attachments) onto the
 *  transcript-rendered conversation. Additive only — never replaces transcript
 *  content (which stays the canonical, re-ingestable history):
 *  - Matched turns (`native_id` set) hang their attachments on the rendered
 *    user message. Aligned from the end, so older turns that predate this
 *    feature (no row) simply keep no attachments instead of mis-grabbing them.
 *  - Pending turns (`native_id` null — the agent never logged them, e.g. a
 *    failed send) render standalone so the message survives reload + retry. */
/** Copy a turn's run timing onto its rendered user message, if present. */
function applyTurnTiming(item: Extract<ChatItem, { kind: "user_message" }>, t: UserTurn): void {
  if (t.started_at != null) item.startedAt = t.started_at;
  if (t.ended_at != null) item.endedAt = t.ended_at;
}

export function applyUserTurns(items: ChatItem[], turns: UserTurn[]): ChatItem[] {
  if (turns.length === 0) return items;

  const matched = turns.filter((t) => t.native_id);
  const pending = turns.filter((t) => !t.native_id);
  const result = items.map((it) => ({ ...it }));

  const userIdxs: number[] = [];
  result.forEach((it, i) => {
    if (it.kind === "user_message") userIdxs.push(i);
  });

  // End-align matched turns to the trailing rendered user messages.
  const n = Math.min(matched.length, userIdxs.length);
  for (let k = 1; k <= n; k++) {
    const t = matched[matched.length - k];
    const item = result[userIdxs[userIdxs.length - k]];
    if (item.kind === "user_message") {
      applyTurnTiming(item, t);
      if (t.attachments.length > 0) {
        item.attachments = t.attachments;
        // Render the clean text the user actually typed (what the live render
        // showed) rather than the transcript's copy, which the runner padded
        // with `Attached file: <path>` reference lines. The stored turn text is
        // verbatim what was sent, so it matches the optimistic render exactly.
        // Prefix-guard so a mis-aligned match can't rewrite an unrelated message.
        if (item.text.startsWith(t.text)) {
          item.text = t.text;
        }
      }
    }
  }

  for (const t of pending) {
    const item: ChatItem = { kind: "user_message", text: t.text };
    if (t.attachments.length > 0) item.attachments = t.attachments;
    applyTurnTiming(item, t);
    result.push(item);
  }

  return result;
}

/** Locate a `prev` item within the freshly-rebuilt transcript so a carried-over
 *  follow-up can be re-anchored next to it. Scans forward from `from` and
 *  returns the FIRST match, so callers advancing `from` in step with a forward
 *  walk through `prev` get a monotonically-advancing anchor: duplicate text
 *  (e.g. repeated "OK" acknowledgements) then resolves to the occurrence at this
 *  point in the turn rather than the last one in the log. Tool calls match on
 *  their stable id; user/agent messages on exact text; other kinds aren't
 *  reliable anchors. Returns -1 when not found at or after `from`. */
function locateAnchor(rebuilt: ChatItem[], item: ChatItem, from: number): number {
  if (item.kind !== "tool_call" && item.kind !== "user_message" && item.kind !== "agent_message") {
    return -1;
  }
  const textOf = (r: ChatItem): string | undefined =>
    r.kind === "user_message" || r.kind === "agent_message" ? r.text : undefined;
  for (let i = Math.max(from, 0); i < rebuilt.length; i += 1) {
    const r = rebuilt[i];
    if (item.kind === "tool_call") {
      if (r.kind === "tool_call" && r.id === item.id) return i;
    } else if (r.kind === item.kind && textOf(r) === item.text) {
      return i;
    }
  }
  return -1;
}

/** Carry forward store-only items — optimistic mid-turn follow-ups
 *  (`queued_message`) and user-invoked command output (`command_output`
 *  notices: `/doctor`, `/cost`, a blocked-command explanation) — onto a log
 *  just rebuilt from canonical records. Neither ever lands in the transcript,
 *  so a plain rebuild would drop them; re-inserting keeps them visible for the
 *  session (until a full transcript reload). Command output always carries;
 *  queued follow-ups drop once a real turn accounts for them.
 *
 *  Drops any follow-up the rebuilt conversation already accounts
 *  for — its text (or first attachment path) now appears in a user message,
 *  whether the follow-up was delivered live (claude) or coalesced (per-turn) —
 *  mirroring the backend matcher's substring association.
 *
 *  A follow-up that isn't in the transcript is re-inserted at its injection
 *  point: right after the nearest preceding item we can still locate in the
 *  rebuilt log. This keeps a live-injected message (which claude does not
 *  persist as its own mid-turn record) in its place within the turn instead of
 *  jumping to the bottom below the answer it prompted. Follow-ups with no
 *  locatable anchor (e.g. an attachment-only one with no needle) fall to the
 *  end, held there until they can be matched. */
export function carryForwardStoreOnly(rebuilt: ChatItem[], prev: ChatItem[]): ChatItem[] {
  const matched = (q: Extract<ChatItem, { kind: "queued_message" }>): boolean => {
    const needle = q.text || q.attachments?.[0];
    if (!needle) return false;
    return rebuilt.some(
      (r) =>
        r.kind === "user_message" &&
        (r.text.includes(needle) || (r.attachments?.includes(needle) ?? false)),
    );
  };

  // Walk prev, tracking the rebuilt-index of the most recent locatable item.
  // Each store-only item is bucketed to insert after that anchor; -1 means no
  // anchor was found yet, so it falls to the end. Searching forward from
  // `anchor + 1` keeps the anchor advancing in lockstep with the walk, so
  // repeated text resolves to the right occurrence.
  const insertAfter = new Map<number, ChatItem[]>();
  let anchor = -1;
  const carry = (it: ChatItem) => {
    const bucket = insertAfter.get(anchor) ?? [];
    bucket.push(it);
    insertAfter.set(anchor, bucket);
  };
  for (const it of prev) {
    if (it.kind === "queued_message") {
      // Drop once a real turn echoes it; otherwise hold it in place.
      if (!matched(it)) carry(it);
    } else if (it.kind === "notice" && it.subtype === "command_output") {
      // Command output lives only in the store — always re-insert it.
      carry(it);
    } else {
      const idx = locateAnchor(rebuilt, it, anchor + 1);
      if (idx >= 0) anchor = idx;
    }
  }
  if (insertAfter.size === 0) return rebuilt;

  const result: ChatItem[] = [];
  rebuilt.forEach((r, i) => {
    result.push(r);
    const add = insertAfter.get(i);
    if (add) result.push(...add);
  });
  const tail = insertAfter.get(-1);
  if (tail) result.push(...tail);
  return result;
}

/** Apply one raw event to an agent's log via its provider adapter. Pure: it
 *  returns the state patch plus a `turnEnded` flag so the caller can fire any
 *  side effects (e.g. the completion chime). Catches adapter throws so a single
 *  malformed event can't poison the whole log. */
export function applyEvent(
  state: AppState,
  agentId: string,
  rawEvent: RawEvent,
): { patch: Partial<AppState>; turnEnded: boolean } {
  const adapter = getAdapter(providerFor(state, agentId));
  const prev = state.managedLogs[agentId] ?? [];
  let next: ChatItem[];
  try {
    next = adapter.reduce(prev, rawEvent);
  } catch (err) {
    console.error("[adapters] reduce threw", {
      provider: adapter.id,
      type: rawEvent.type,
      err,
    });
    return { patch: {}, turnEnded: false };
  }
  if (next === prev) return { patch: {}, turnEnded: false };

  // `result` events signal turn end for claude; mirror that state on the
  // store so the composer re-enables. Adapter-agnostic: any notice with
  // subtype "turn_end" appended this tick clears managedBusy. The `next !== prev`
  // guard above means this is true exactly once per turn-end.
  const turnEnded =
    next.length > prev.length &&
    next[next.length - 1]?.kind === "notice" &&
    (next[next.length - 1] as { subtype?: string }).subtype === "turn_end";

  return {
    turnEnded,
    patch: {
      managedLogs: { ...state.managedLogs, [agentId]: next },
      managedBusy: turnEnded ? { ...state.managedBusy, [agentId]: false } : state.managedBusy,
      managedBusyLabel: turnEnded
        ? { ...state.managedBusyLabel, [agentId]: undefined }
        : state.managedBusyLabel,
    },
  };
}
