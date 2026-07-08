import type { AgentUsage } from "@/adapters/usage";
import { localDay } from "@/util/format";
import { dbUpsert } from "./db";

// Daily token-usage snapshots (usage_daily table): one row per (workspace,
// local day) holding the session's CUMULATIVE totals as of the last fold that
// day. Cumulative — not per-day deltas — because some providers (codex) only
// report running totals; a day's spend is the difference between consecutive
// snapshots. Written opportunistically from every place usage is re-folded
// from session_records, so history accrues as the app is used.

// Last written fingerprint per workspace, so re-folds that didn't change the
// totals (the common refresh case) never touch the DB.
const lastWritten = new Map<string, string>();

/** Upsert today's cumulative usage snapshot for a workspace. Fire-and-forget:
 *  failures are logged, never thrown — stats are best-effort by design. No-op
 *  when the project is unknown or the totals haven't changed since the last
 *  write this session. */
export function recordUsageSnapshot(
  workspaceId: string,
  projectId: string | undefined,
  usage: AgentUsage,
): void {
  if (!workspaceId || !projectId) return;
  const now = Date.now();
  const day = localDay(now);
  const fingerprint = [
    day,
    usage.inputTokens,
    usage.outputTokens,
    usage.cacheReadTokens,
    usage.cacheWriteTokens,
    usage.costUsd,
  ].join("|");
  if (lastWritten.get(workspaceId) === fingerprint) return;
  lastWritten.set(workspaceId, fingerprint);
  dbUpsert(
    "usage_daily",
    {
      workspace_id: workspaceId,
      project_id: projectId,
      day,
      input_tokens: usage.inputTokens,
      output_tokens: usage.outputTokens,
      cache_read_tokens: usage.cacheReadTokens,
      cache_write_tokens: usage.cacheWriteTokens,
      cost_usd: usage.costUsd,
      updated_at: now,
    },
    "workspace_id,day",
  ).catch((err) => console.error("usage snapshot failed", err));
}
