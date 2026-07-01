// Pure helpers backing the store: transcript/event reduction, usage
// persistence, and small lookups. Kept out of store.ts so the store module is
// just state + actions, and so these can be unit-tested directly. They depend
// on the store only for its type shape (AppState/DraftAgent) — a type-only
// import, erased at compile time, so there's no runtime cycle.

import { type ChatItem, getAdapter, type RawEvent } from "./adapters";
import { hasUsage, usageFromRecords } from "./adapters/usage";
import { api, type SessionRecord, type UserTurn, type Workspace } from "./api";
import { commandsFor } from "./data/slashCommands";
import type { AppState, DraftAgent } from "./store";

export function providerFor(state: AppState, agentId: string): string | undefined {
  return state.workspace?.agents.find((a) => a.id === agentId)?.provider;
}

/** If `text` is a `/<name>` matching a known passthrough command for the
 *  given provider, return its bare name; otherwise null. The result is
 *  used both to swap the optimistic user_message for a slash_command
 *  notice and to set a busy label. */
export function passthroughSlashName(providerId: string | undefined, text: string): string | null {
  if (!providerId || !text.startsWith("/")) return null;
  const first = text.split(/\s/)[0].slice(1);
  const match = commandsFor(providerId).find((c) => c.kind === "passthrough" && c.name === first);
  return match ? match.name : null;
}

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

/** Carry forward optimistic mid-turn follow-ups (`queued_message`) onto a log
 *  just rebuilt from canonical records, so they don't blink out before the
 *  transcript catches up. Drops any the rebuilt conversation already accounts
 *  for — its text (or first attachment path) now appears in a user message,
 *  whether the follow-up was delivered live (claude) or coalesced (per-turn) —
 *  mirroring the backend matcher's substring association. An attachment-only
 *  follow-up with no needle is kept until it can be matched. */
export function carryForwardQueued(rebuilt: ChatItem[], prev: ChatItem[]): ChatItem[] {
  const stillQueued = prev.filter((it) => {
    if (it.kind !== "queued_message") return false;
    const needle = it.text || it.attachments?.[0];
    if (!needle) return true;
    return !rebuilt.some(
      (r) =>
        r.kind === "user_message" &&
        (r.text.includes(needle) || (r.attachments?.includes(needle) ?? false)),
    );
  });
  return [...rebuilt, ...stillQueued];
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

/** Cursor reports token usage only on its live `result` event (never on disk),
 *  so persist that event into session_records (`live_compiled`) when it lands —
 *  then usage folds from records like every other agent, surviving restarts.
 *  Idempotent on the event's `request_id`; after persisting, re-fold so the
 *  gauge updates this turn rather than only on the next records refresh. */
export async function persistLiveUsage(
  get: () => AppState,
  set: (patch: Partial<AppState>) => void,
  agentId: string,
  rawEvent: RawEvent,
): Promise<void> {
  const provider = providerFor(get(), agentId);
  const adapter = getAdapter(provider);
  if (!adapter.persistLiveUsage || !adapter.extractUsage) return;
  if (!adapter.extractUsage(rawEvent)) return; // nothing to persist this event
  // Idempotency key: cursor's `request_id`, else a stable per-event id (opencode
  // nests a unique `prt_…` part id), else a timestamp as a last resort.
  const part =
    typeof rawEvent.part === "object" && rawEvent.part
      ? (rawEvent.part as Record<string, unknown>)
      : undefined;
  const partId = part && typeof part.id === "string" ? part.id : undefined;
  const nativeId =
    (typeof rawEvent.request_id === "string" && rawEvent.request_id) ||
    partId ||
    `usage:${Date.now()}`;
  try {
    await api.appendLiveRecord(agentId, provider ?? adapter.id, nativeId, rawEvent);
    const records = await api.readSessionRecords(agentId);
    const usage = usageFromRecords(provider, records);
    if (hasUsage(usage)) {
      set({ usage: { ...get().usage, [agentId]: usage } });
    }
  } catch {
    // Non-critical: the next records refresh or restart re-folds it.
  }
}

/** A per-turn agent captures its session id on its first turn (e.g. agy reads
 *  it from disk at turn-end), but the id only reaches the live frontend via a
 *  full `getWorkspace`. True when an agent's turn just landed yet its session
 *  id is still missing locally — the cue to re-fetch so the Native toggle
 *  unblocks without a reload. False once present, to avoid per-turn re-fetch. */
export function needsSessionIdRefresh(workspace: Workspace | null, agentId: string): boolean {
  const agent = workspace?.agents.find((a) => a.id === agentId);
  return !!agent && !agent.session_id;
}

/** Names already taken by real or draft agents — passed to the backend
 *  name allocator so picks avoid collisions. */
export function usedNames(workspace: Workspace | null, drafts: DraftAgent[]): Set<string> {
  const used = new Set<string>();
  for (const a of workspace?.agents ?? []) used.add(a.name);
  for (const d of drafts) used.add(d.name);
  return used;
}

/** Strip an agent's entries from every ephemeral per-agent map, returning just
 *  the pruned maps as a state patch (the caller layers on workspace /
 *  selectedAgentId). Shared by discard and archive — dropping these is safe
 *  because History re-loads an archived agent's transcript fresh from disk. */
export function dropAgentEntries(state: AppState, id: string): Partial<AppState> {
  const { [id]: _log, ...managedLogs } = state.managedLogs;
  const { [id]: _loading, ...transcriptLoading } = state.transcriptLoading;
  const { [id]: _loaded, ...transcriptLoaded } = state.transcriptLoaded;
  const { [id]: _busy, ...managedBusy } = state.managedBusy;
  const { [id]: _started, ...turnStartedAt } = state.turnStartedAt;
  const { [id]: _usage, ...usage } = state.usage;
  const { [id]: _git, ...gitStates } = state.gitStates;
  const { [id]: _short, ...gitShortstats } = state.gitShortstats;
  const { [id]: _pr, ...prStates } = state.prStates;
  const { [id]: _checks, ...prChecks } = state.prChecks;
  const { [id]: _comments, ...prComments } = state.prComments;
  const { [id]: _seed, ...composerSeeds } = state.composerSeeds;
  const { [id]: _draft, ...composerDrafts } = state.composerDrafts;
  const { [id]: _delegation, ...gitDelegations } = state.gitDelegations;
  // Drop the unseen-results flag too: otherwise archiving/discarding an agent
  // that finished while unviewed leaves an orphaned key behind with no row to
  // select, which would keep the app-icon badge count nonzero forever.
  const { [id]: _seen, ...unseenResults } = state.unseenResults;
  // Drop the remembered right-rail tab so an archived/discarded agent's UI
  // state doesn't outlive it as a stale key for the rest of the session.
  const { [id]: _tab, ...rightPanelTabs } = state.rightPanelTabs;
  return {
    managedLogs,
    transcriptLoading,
    transcriptLoaded,
    managedBusy,
    turnStartedAt,
    usage,
    gitStates,
    gitShortstats,
    prStates,
    prChecks,
    prComments,
    composerSeeds,
    composerDrafts,
    gitDelegations,
    unseenResults,
    rightPanelTabs,
  };
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export async function sendWhenAgentReady(send: () => Promise<void>) {
  let lastError: unknown;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      await send();
      return;
    } catch (e) {
      lastError = e;
      if (!String(e).includes("agent not found")) {
        throw e;
      }
      await sleep(250);
    }
  }
  throw lastError;
}
