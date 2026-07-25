import type { UsageSnapshot } from "@/adapters/usage";
import { localDay } from "@/util/format";
import { dbUpsert } from "./db";

// Daily token-usage snapshots (usage_daily table): one row per (workspace,
// local day) holding the session's CUMULATIVE spend as of the last aggregation
// that day. Cumulative — not per-day deltas — because a session's ledger is
// only ever rebuilt in full from session_records; a day's spend is the
// difference between consecutive snapshots. Written opportunistically from
// every place usage is re-aggregated, so history accrues as the app is used.

// Last written fingerprint per workspace, so re-reads that didn't change the
// totals (the common refresh case) never touch the DB.
const lastWritten = new Map<string, string>();

/** Upsert today's cumulative usage snapshot for a workspace. Fire-and-forget:
 *  failures are logged, never thrown — stats are best-effort by design. No-op
 *  when the project is unknown or the totals haven't changed since the last
 *  write this session. */
export function recordUsageSnapshot(
  workspaceId: string,
  projectId: string | undefined,
  usage: UsageSnapshot,
): void {
  if (!workspaceId || !projectId) return;
  const now = Date.now();
  const day = localDay(now);
  const { tokens, costUsd } = usage.spend;
  // `cost_usd` is NOT NULL DEFAULT 0, and a bound NULL does not fall back to a
  // column default — it fails the constraint. A snapshot's cost is null whenever
  // no agent priced its own calls (claude, codex, cursor), which is most
  // sessions, so this is the difference between recording their history and
  // silently recording none of it. The column already reads 0 as "unpriced".
  const cost = costUsd ?? 0;
  const fingerprint = [
    day,
    tokens.input,
    tokens.output,
    tokens.cacheRead,
    tokens.cacheWrite,
    cost,
  ].join("|");
  if (lastWritten.get(workspaceId) === fingerprint) return;
  lastWritten.set(workspaceId, fingerprint);
  dbUpsert(
    "usage_daily",
    {
      workspace_id: workspaceId,
      project_id: projectId,
      day,
      input_tokens: tokens.input,
      output_tokens: tokens.output,
      cache_read_tokens: tokens.cacheRead,
      cache_write_tokens: tokens.cacheWrite,
      cost_usd: cost,
      updated_at: now,
    },
    "workspace_id,day",
  ).catch((err) => console.error("usage snapshot failed", err));
}
