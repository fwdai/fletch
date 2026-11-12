// Usage persistence: fold a live-only usage event (cursor's `result`) into
// session_records so token usage survives restarts and folds like every other
// agent, and record a workspace's spend for the day. Plus the one thing that is
// not persisted: the usage of a turn still in flight, folded over the records'
// snapshot until the records catch up.

import { getAdapter, type RawEvent } from "../adapters";
import {
  aggregate,
  hasUsage,
  type Spend,
  sessionSpend,
  type UsageEvent,
  type UsageSnapshot,
  unspentSnapshot,
  usageFromRecords,
  withLiveTurn,
  withSupersededSpend,
} from "../adapters/usage";
import { api } from "../api";
import { recordUsageSnapshot } from "../storage/usageDaily";
import type { AppState } from "../store";
import { agentRecord, providerFor } from "./agentLookups";

/** Record what a workspace has spent: `usage`, its current conversation's,
 *  plus what the sessions it superseded spent (see `withSupersededSpend`).
 *  Returns that total. */
export async function recordWorkspaceUsage(
  workspaceId: string,
  projectId: string | undefined,
  usage: UsageSnapshot,
): Promise<UsageSnapshot> {
  const total = withSupersededSpend(usage, await supersededSpend(workspaceId));
  if (hasUsage(total)) recordUsageSnapshot(workspaceId, projectId, total);
  return total;
}

/** Each superseded session's spend, by session id. A superseded session never
 *  changes again (nothing ingests into it), so it is folded once; ids are
 *  unique across workspaces and hosts, so one map serves them all. */
const foldedSessions = new Map<string, Spend>();

/** What each of `workspaceId`'s superseded sessions spent. The host lists
 *  them all but sends records only for those not folded yet, so once they are,
 *  this is one query that returns ids. A host that can't list them (one that
 *  predates rewind) has none. */
async function supersededSpend(workspaceId: string): Promise<Spend[]> {
  const sessions = await api
    .readSupersededRecords(workspaceId, [...foldedSessions.keys()])
    .catch(() => []);
  return sessions.map(({ session_id, records }) => {
    let spend = foldedSessions.get(session_id);
    if (!spend) {
      spend = sessionSpend(records);
      foldedSessions.set(session_id, spend);
    }
    return spend;
  });
}

/** Cursor and OpenCode report token usage only on their live stream (never on
 *  disk), so persist that event into session_records (`live_compiled`) when it
 *  lands — usage then aggregates from records like every other agent, surviving
 *  restarts. Idempotent on the event's own id; after persisting, re-aggregate
 *  so the gauge updates this turn rather than on the next records refresh. */
export async function persistLiveUsage(
  get: () => AppState,
  set: (patch: Partial<AppState>) => void,
  agentId: string,
  rawEvent: RawEvent,
): Promise<void> {
  const provider = providerFor(get(), agentId);
  const adapter = getAdapter(provider);
  if (!adapter.persistLiveUsage || !adapter.usageEvents) return;
  if (adapter.usageEvents(rawEvent).length === 0) return; // nothing to persist
  // Idempotency key: cursor's `request_id`, else a stable per-event id (opencode
  // nests a unique `prt_…` part id), else a digest of the event. A timestamp
  // used to be the last resort, which made the row non-idempotent: a redelivered
  // event landed twice and its tokens were counted twice.
  const part =
    typeof rawEvent.part === "object" && rawEvent.part
      ? (rawEvent.part as Record<string, unknown>)
      : undefined;
  const partId = part && typeof part.id === "string" ? part.id : undefined;
  const nativeId =
    (typeof rawEvent.request_id === "string" && rawEvent.request_id) ||
    partId ||
    `usage:${digest(rawEvent)}`;
  try {
    await api.appendLiveRecord(agentId, provider ?? adapter.id, nativeId, rawEvent);
    const records = await api.readSessionRecords(agentId);
    const usage = usageFromRecords(provider, records);
    if (hasUsage(usage)) {
      set({ usage: { ...get().usage, [agentId]: usage } });
      // Via agentRecord, not the workspace snapshot: an off-sidebar chat's
      // spend belongs to its project like anyone else's.
      await recordWorkspaceUsage(agentId, agentRecord(get(), agentId)?.project_id, usage);
    }
  } catch {
    // Non-critical: the next records refresh or restart re-aggregates it.
  }
}

/** What each agent's live stream has reported beyond its records: `base`, the
 *  snapshot the records last gave, and the calls streamed since. The store
 *  holds only the two folded together. `seen` counts every call ever folded,
 *  so a point in the stream can be named (`beginUsageRead`) and still mean the
 *  same thing after earlier calls were handed to the records.
 *
 *  Keyed by agent id, which is a recycled place name: `created` says which
 *  agent of that name the turn belongs to, so a new one never folds onto what
 *  a discarded or another host's namesake left here. */
interface LiveTurn {
  created: string | undefined;
  base: UsageSnapshot;
  events: UsageEvent[];
  seen: number;
}

const liveTurns = new Map<string, LiveTurn>();

/** One read of an agent's records, as it stood when it began: its place among
 *  all reads, and how far the agent's live stream had got. */
export interface UsageRead {
  order: number;
  seen: number;
}

let reads = 0;
/** Per agent, the latest read whose snapshot was settled. */
const settledReads = new Map<string, number>();

/** Fold the usage a live event reports (`liveUsageEvents`) into the agent's
 *  snapshot, so the gauge moves while the turn runs instead of when its records
 *  land. Nothing is persisted or recorded as spend: the records are the ledger,
 *  and `settleLiveUsage` hands the snapshot back to them. */
export function foldLiveUsage(
  get: () => AppState,
  set: (patch: Partial<AppState>) => void,
  agentId: string,
  rawEvent: RawEvent,
): void {
  const agent = agentRecord(get(), agentId);
  const events = getAdapter(agent?.provider).liveUsageEvents?.(rawEvent) ?? [];
  if (events.length === 0) return;
  let turn = liveTurns.get(agentId);
  if (turn?.created !== agent?.created_at) turn = undefined;
  if (!turn) {
    turn = {
      created: agent?.created_at,
      // A call need not name its model; until one does, the window is the
      // spawn model's — the same one the gauge showed before the call.
      base: get().usage[agentId] ?? unspentSnapshot(agent?.model),
      events: [],
      seen: 0,
    };
    liveTurns.set(agentId, turn);
  }
  turn.events.push(...events);
  turn.seen += events.length;
  set({
    usage: { ...get().usage, [agentId]: withLiveTurn(turn.base, aggregate(turn.events)) },
  });
}

/** Note that a read of the agent's records is starting. Call it BEFORE the
 *  read and pass the result to `settleLiveUsage` with the snapshot it gave:
 *  the read is async, so calls can stream in while it runs, and another read
 *  can start after it and land first. */
export function beginUsageRead(agentId: string): UsageRead {
  reads += 1;
  return { order: reads, seen: liveTurns.get(agentId)?.seen ?? 0 };
}

/** Settle the agent's usage on the records: `recorded` is the snapshot `read`
 *  gave. Returns the snapshot to put in the store, or null when it must be
 *  left as it is. Call it wherever a snapshot built from records is written.
 *
 *  Null is for a read that began before one already settled. Reads overlap and
 *  land out of order; the earlier one holds the older records, and storing it
 *  would roll the snapshot back — a compaction undone, a call lost.
 *
 *  Otherwise the calls streamed before the read began are the records' now and
 *  are dropped, or the next fold would count them twice. Calls streamed since
 *  are not in what it read, and stay folded over `recorded`. */
export function settleLiveUsage(
  agentId: string,
  recorded: UsageSnapshot,
  read: UsageRead,
): UsageSnapshot | null {
  if (read.order < (settledReads.get(agentId) ?? 0)) return null;
  settledReads.set(agentId, read.order);
  const turn = liveTurns.get(agentId);
  if (!turn) return recorded;
  const since = Math.min(turn.events.length, turn.seen - read.seen);
  turn.base = recorded;
  turn.events = since > 0 ? turn.events.slice(-since) : [];
  return withLiveTurn(recorded, aggregate(turn.events));
}

/** FNV-1a over the event JSON — a short, stable id for an event that carries
 *  none of its own, so re-delivering it lands on the same row. */
function digest(event: RawEvent): string {
  const json = JSON.stringify(event);
  let hash = 0x811c9dc5;
  for (let i = 0; i < json.length; i += 1) {
    hash ^= json.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(36);
}
