import { hasUsage, usageFromRecords } from "@/adapters/usage";
import { api } from "@/api";
import { dbQuery } from "@/storage/db";
import { recordUsageSnapshot } from "@/storage/usageDaily";

// Data layer for the Project Pulse block. Everything here reads what the app
// already persists (session_user_turns, workspaces, worktrees) via SELECT-only
// raw queries; nothing is recorded on open except the opportunistic usage
// snapshots seeded by `loadPulseUsage`.

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
    dbQuery<{ day: string; n: number }>(
      `SELECT date(wt.pr_opened_at/1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS n
       FROM worktrees wt JOIN workspaces w ON w.id = wt.workspace_id
       WHERE w.project_id = ? AND wt.pr_opened_at >= ?
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
          tokens += usage.inputTokens + usage.outputTokens;
          costUsd += usage.costUsd;
          recordUsageSnapshot(r.id, projectId, usage);
        } catch {
          // Unreadable session (e.g. cleaned-up archive) — skip, don't abort.
        }
      }),
    );
  }
  return { tokens, costUsd };
}
