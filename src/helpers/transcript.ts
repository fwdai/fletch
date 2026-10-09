// Transcript / log reduction: rendering canonical session_records into chat
// items, carrying forward store-only items across a rebuild, and applying a
// single live event. User turns merge in through `mergeTurns`.

import { type ChatItem, getAdapter, type RawEvent } from "../adapters";
import type { SessionRecord } from "../api";
import type { AppState } from "../store";
import { providerFor } from "./agentLookups";
import { hasSentTurn } from "./mirrorTurn";

/** Render canonical `session_records` (verbatim per-provider transcript
 *  bodies) into chat items via the same pipeline as on-disk replay:
 *  `normalizeTranscript` → `reduce`. Every item carries the `recordSeq` and
 *  `recordSession` of the record it came from. Defensive: a malformed body or
 *  an adapter throw degrades gracefully instead of failing the whole restore. */
export function reduceRecords(provider: string | undefined, records: SessionRecord[]): ChatItem[] {
  const adapter = getAdapter(provider);
  let rawEvents: RawEvent[];
  try {
    rawEvents = adapter.normalizeTranscript(
      records.map((r) => r.body),
      records.map((r) => r.seq),
      records.map((r) => r.session_id),
    );
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

/** Locate a `prev` item within the freshly-rebuilt transcript so a carried-over
 *  store-only item can be re-anchored next to it. Scans forward from `from` and
 *  returns the FIRST match, so callers advancing `from` in step with a forward
 *  walk through `prev` get a monotonically-advancing anchor: duplicate text
 *  (e.g. repeated "OK" acknowledgements) then resolves to the occurrence at this
 *  point in the turn rather than the last one in the log. Tool calls match on
 *  their stable id; user/agent messages and reasoning on exact text; other
 *  kinds aren't reliable anchors. Returns -1 when not found at or after `from`. */
function locateAnchor(rebuilt: ChatItem[], item: ChatItem, from: number): number {
  const isReasoning = item.kind === "notice" && item.subtype === "reasoning";
  if (
    item.kind !== "tool_call" &&
    item.kind !== "user_message" &&
    item.kind !== "agent_message" &&
    !isReasoning
  ) {
    return -1;
  }
  const textOf = (r: ChatItem): string | undefined =>
    r.kind === "user_message" ||
    r.kind === "agent_message" ||
    (r.kind === "notice" && r.subtype === "reasoning")
      ? r.text
      : undefined;
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

/** Carry forward store-only items — optimistic mid-turn follow-ups,
 *  readable Codex reasoning awaiting its durable compiled-record write,
 *  (`queued_message`) and user-invoked command output (`command_output`
 *  notices: `/doctor`, `/cost`, a blocked-command explanation) — onto a log
 *  just rebuilt from canonical records. Neither ever lands in the transcript,
 *  so a plain rebuild would drop them; re-inserting keeps them visible for the
 *  session (until a full transcript reload). Command output always carries;
 *  queued follow-ups drop once the rebuilt log draws their turn.
 *
 *  Drops any follow-up the rebuilt conversation already draws: an item there
 *  carries its `turnId`, because its turn row was merged in (`mergeUserTurns`)
 *  — onto its echo, or standalone while it awaits one.
 *
 *  A follow-up the rebuilt log doesn't draw is re-inserted at its injection
 *  point: right after the nearest preceding item we can still locate in the
 *  rebuilt log, so it keeps its place within the turn instead of jumping to the
 *  bottom below the answer it prompted. Follow-ups with no locatable anchor
 *  fall to the end. */
export function carryForwardStoreOnly(rebuilt: ChatItem[], prev: ChatItem[]): ChatItem[] {
  const matched = (q: Extract<ChatItem, { kind: "queued_message" }>): boolean =>
    q.turnId !== undefined && hasSentTurn(rebuilt, q.turnId);

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
    } else if (it.kind === "notice" && it.subtype === "reasoning") {
      // A persisted compiled reasoning record may already have restored it. If
      // not, keep the live row in place while that asynchronous write settles.
      const idx = locateAnchor(rebuilt, it, anchor + 1);
      if (idx >= 0) anchor = idx;
      else carry(it);
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
): { patch: Partial<AppState>; turnEnded: boolean; turnFailed: boolean } {
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
    return { patch: {}, turnEnded: false, turnFailed: false };
  }
  if (next === prev) return { patch: {}, turnEnded: false, turnFailed: false };

  // Adapter-agnostic turn-end detection, for the caller's side effects (the
  // completion chime, the unseen-results dot): any notice with subtype
  // "turn_end" appended this tick. The `next !== prev` guard above means this
  // is true exactly once per turn-end. The busy state itself is not touched
  // here — the backend's `agent:status` owns it.
  const last = next[next.length - 1] as { kind?: string; subtype?: string; text?: string };
  const turnEnded =
    next.length > prev.length && last?.kind === "notice" && last.subtype === "turn_end";
  // Adapters close a failed turn with the same notice, text "error".
  const turnFailed = turnEnded && last.text === "error";

  return {
    turnEnded,
    turnFailed,
    patch: { managedLogs: { ...state.managedLogs, [agentId]: next } },
  };
}
