import { hasUsage, usageFromRecords } from "@/adapters/usage";
import { api } from "@/api";
import { dbQuery } from "@/storage/db";
import { recordUsageSnapshot } from "@/storage/usageDaily";
import {
  dailySpend,
  type MergeStats,
  mergeStats,
  type PrRow,
  type SpendDay,
  type UsageRow,
} from "./derive";

// Data layer for the Activity tab. Everything here reads what the app already
// persists (session_user_turns, workspaces, worktrees, worktree_prs,
// usage_daily, roadmap_items) via SELECT-only raw queries; nothing is recorded
// on open except the opportunistic usage snapshots seeded by `loadPulseUsage`.
//
// Each loader is a query plus a call into `derive` — the shaping lives there so
// it can be tested without a database.

/** Per-local-day counts feeding the heatmap and its tooltip. */
export interface PulseActivity {
  /** User turns sent to agents of this project — the heatmap intensity. */
  turns: Record<string, number>;
  /** Agents launched. */
  agents: Record<string, number>;
  /** PRs opened (only days observed since PR-time stamping shipped). */
  prs: Record<string, number>;
}

export interface PulseTotals {
  agents: number;
  agents7d: number;
  prsOpened: number;
  prsMerged: number;
  additions: number;
  deletions: number;
}

export interface PulseUsage {
  /** Input + output tokens across every session of the project. */
  tokens: number;
  /** Summed cost; 0 when no provider in the project reports cost. */
  costUsd: number;
}

const DAY_MS = 86_400_000;

const toDayMap = (rows: Array<{ day: string; n: number }>): Record<string, number> => {
  const out: Record<string, number> = {};
  for (const r of rows) if (r.day) out[r.day] = r.n;
  return out;
};

/** The three per-day series, bucketed by the user's local calendar. */
export async function loadPulseActivity(
  projectId: string,
  sinceMs: number,
): Promise<PulseActivity> {
  const [turns, agents, prs] = await Promise.all([
    dbQuery<{ day: string; n: number }>(
      `SELECT date(t.created_at/1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS n
       FROM session_user_turns t
       JOIN sessions s ON s.id = t.session_id
       JOIN workspaces w ON w.id = s.workspace_id
       WHERE w.project_id = ? AND t.created_at >= ?
       GROUP BY day`,
      [projectId, sinceMs],
    ),
    dbQuery<{ day: string; n: number }>(
      `SELECT date(created_at/1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS n
       FROM workspaces WHERE project_id = ? AND created_at >= ?
       GROUP BY day`,
      [projectId, sinceMs],
    ),
    // Every PR the project's checkouts have opened — from the history log, not
    // `worktrees.pr_*`. Those columns hold only a checkout's CURRENT binding, so
    // a workspace that merged and then opened a follow-up counted once, and the
    // earlier PR's `pr_opened_at` was overwritten out of the series entirely.
    dbQuery<{ day: string; n: number }>(
      `SELECT date(p.opened_at/1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS n
       FROM worktree_prs p JOIN workspaces w ON w.id = p.workspace_id
       WHERE w.project_id = ? AND p.opened_at >= ?
       GROUP BY day`,
      [projectId, sinceMs],
    ),
  ]);
  return { turns: toDayMap(turns), agents: toDayMap(agents), prs: toDayMap(prs) };
}

/** Lifetime headline numbers for the tile row. */
export async function loadPulseTotals(projectId: string, nowMs: number): Promise<PulseTotals> {
  const weekAgo = nowMs - 7 * DAY_MS;
  const [agentRows, repoRows] = await Promise.all([
    dbQuery<{ n: number; recent: number }>(
      `SELECT COUNT(*) AS n,
              COALESCE(SUM(CASE WHEN created_at >= ? THEN 1 ELSE 0 END), 0) AS recent
       FROM workspaces WHERE project_id = ?`,
      [weekAgo, projectId],
    ),
    dbQuery<{ prs: number; merged: number; adds: number; dels: number }>(
      `SELECT COUNT(wt.pr_number) AS prs,
              COALESCE(SUM(CASE WHEN wt.pr_merged_at IS NOT NULL THEN 1 ELSE 0 END), 0) AS merged,
              COALESCE(SUM(wt.diff_additions), 0) AS adds,
              COALESCE(SUM(wt.diff_deletions), 0) AS dels
       FROM worktrees wt JOIN workspaces w ON w.id = wt.workspace_id
       WHERE w.project_id = ?`,
      [projectId],
    ),
  ]);
  return {
    agents: agentRows[0]?.n ?? 0,
    agents7d: agentRows[0]?.recent ?? 0,
    prsOpened: repoRows[0]?.prs ?? 0,
    prsMerged: repoRows[0]?.merged ?? 0,
    additions: repoRows[0]?.adds ?? 0,
    deletions: repoRows[0]?.dels ?? 0,
  };
}

/** Fold every session of the project into a token/cost total. Reads each
 *  agent's transcript records, so it runs lazily behind the tile shimmer;
 *  folded totals are also snapshotted into usage_daily, seeding per-day
 *  history for the whole project. Per-agent failures are skipped — the total
 *  is best-effort over what's readable. */
export async function loadPulseUsage(projectId: string): Promise<PulseUsage> {
  const rows = await dbQuery<{ id: string; provider: string | null }>(
    `SELECT w.id AS id,
            (SELECT s.provider FROM sessions s WHERE s.workspace_id = w.id
             ORDER BY s.created_at DESC LIMIT 1) AS provider
     FROM workspaces w WHERE w.project_id = ?`,
    [projectId],
  );
  let tokens = 0;
  let costUsd = 0;
  const CHUNK = 4;
  for (let i = 0; i < rows.length; i += CHUNK) {
    await Promise.all(
      rows.slice(i, i + CHUNK).map(async (r) => {
        try {
          const records = await api.readSessionRecords(r.id);
          if (records.length === 0) return;
          const usage = usageFromRecords(r.provider ?? undefined, records);
          if (!hasUsage(usage)) return;
          // Fresh input + output: the tokens the project actually generated.
          // Cache reads are excluded on purpose — the same cached prefix is
          // re-read every turn, so including them would make a long session
          // look like an order of magnitude more work than it was.
          tokens += usage.spend.tokens.input + usage.spend.tokens.output;
          costUsd += usage.spend.costUsd ?? 0;
          recordUsageSnapshot(r.id, projectId, usage);
        } catch {
          // Unreadable session (e.g. cleaned-up archive) — skip, don't abort.
        }
      }),
    );
  }
  return { tokens, costUsd };
}

/** Open→merge times and the opened/merged trend, from the append-only PR log.
 *
 *  `worktree_prs` (migration 0025) back-seeded itself from the existing
 *  bindings, so this covers the project's whole history rather than starting
 *  at install — unlike the usage series below. Every PR the project has is a
 *  few hundred rows at worst, so the whole log is folded in JS rather than
 *  asking SQLite for a median it has no percentile function for. */
export async function loadMergeStats(
  projectId: string,
  nowMs: number,
  weeks: number,
): Promise<MergeStats> {
  const rows = await dbQuery<PrRow>(
    `SELECT p.opened_at AS opened_at, p.merged_at AS merged_at, p.state AS state
       FROM worktree_prs p JOIN workspaces w ON w.id = p.workspace_id
      WHERE w.project_id = ?`,
    [projectId],
  );
  return mergeStats(rows, nowMs, weeks);
}

/** Per-day token/dollar spend over `days`.
 *
 *  Reads the project's ENTIRE `usage_daily` history, not just the window: the
 *  rows are cumulative, so the first day in range needs its predecessor to
 *  produce a delta at all. One row per (workspace, day) keeps that cheap. */
export async function loadSpend(projectId: string, days: string[]): Promise<SpendDay[]> {
  const rows = await dbQuery<UsageRow>(
    `SELECT workspace_id, day,
            input_tokens + output_tokens AS tokens,
            cost_usd AS cost
       FROM usage_daily WHERE project_id = ?`,
    [projectId],
  );
  return dailySpend(rows, days);
}

/** A shipped roadmap item, as the recent list renders it. */
export interface ShippedItem {
  id: string;
  code: string;
  title: string;
  pr_url: string | null;
  pr_number: number | null;
  /** Ship time, approximated by the last write to the row (see below). */
  updated_at: number;
}

/** The most recently shipped roadmap items.
 *
 *  Ordered by `updated_at`, which is the ship time only until something edits
 *  the row afterwards — `roadmap_items` has no `done_at` column. That is
 *  precise enough for a "recently shipped" list, where a slightly out-of-order
 *  entry costs nothing, and is exactly why there is no shipped-per-week chart
 *  here: a chart would be quietly wrong rather than visibly approximate. */
export async function loadRecentlyShipped(
  projectId: string,
  limit: number,
): Promise<ShippedItem[]> {
  return dbQuery<ShippedItem>(
    `SELECT id, code, title, pr_url, pr_number, updated_at
       FROM roadmap_items
      WHERE project_id = ? AND status = 'done'
      ORDER BY updated_at DESC
      LIMIT ?`,
    [projectId, limit],
  );
}
