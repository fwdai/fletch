// Usage persistence: fold a live-only usage event (cursor's `result`) into
// session_records so token usage survives restarts and folds like every other
// agent.

import { getAdapter, type RawEvent } from "../adapters";
import { hasUsage, usageFromRecords } from "../adapters/usage";
import { api } from "../api";
import { recordUsageSnapshot } from "../storage/usageDaily";
import type { AppState } from "../store";
import { providerFor } from "./agentLookups";

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
      const projectId = get().workspace?.agents.find((a) => a.id === agentId)?.project_id;
      recordUsageSnapshot(agentId, projectId, usage);
    }
  } catch {
    // Non-critical: the next records refresh or restart re-folds it.
  }
}
