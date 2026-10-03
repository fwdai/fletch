// Usage persistence: fold a live-only usage event (cursor's `result`) into
// session_records so token usage survives restarts and folds like every other
// agent, and record a workspace's spend for the day.

import { getAdapter, type RawEvent } from "../adapters";
import {
  hasUsage,
  type Spend,
  sessionSpend,
  type UsageSnapshot,
  usageFromRecords,
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
