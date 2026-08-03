import { localDay, weekStartDay } from "@/util/format";

// Pure math behind the Activity tab's charts. Everything here takes rows
// exactly as they come off `activityData`'s queries and returns exactly what a
// section renders — no React, no DB — so the three genuinely subtle parts are
// unit-testable: cumulative usage snapshots turned into per-day deltas, week
// bucketing across DST, and the distinction between "none" and "not observed".

// ── calendar ranges ───────────────────────────────────────────────────────
// Both build their keys by stepping a noon-anchored Date, so a DST boundary
// can never land a bucket on the wrong day (see `weekStartDay`).

/** The last `n` local day keys, oldest first, ending on the day of `nowMs`. */
export function recentDays(nowMs: number, n: number): string[] {
  const d = new Date(nowMs);
  d.setHours(12, 0, 0, 0);
  d.setDate(d.getDate() - (n - 1));
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    out.push(localDay(d.getTime()));
    d.setDate(d.getDate() + 1);
  }
  return out;
}

/** The last `n` local Monday keys, oldest first, ending with the week
 *  containing `nowMs`. Aligns with `weekStartDay`, which buckets into them. */
export function recentWeeks(nowMs: number, n: number): string[] {
  const d = new Date(nowMs);
  d.setHours(12, 0, 0, 0);
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7) - (n - 1) * 7);
  const out: string[] = [];
  for (let i = 0; i < n; i++) {
    out.push(localDay(d.getTime()));
    d.setDate(d.getDate() + 7);
  }
  return out;
}

// ── time to merge ─────────────────────────────────────────────────────────

/** One `worktree_prs` row, which is append-only and was back-seeded from the
 *  existing bindings — so unlike the usage snapshots, this history is complete
 *  rather than starting at first-open. */
export interface PrRow {
  opened_at: number | null;
  merged_at: number | null;
  /** Serialized `github::PrStatus`: open | merged | closed. */
  state: string;
}

export interface MergeWeek {
  /** Monday of the week, as a local day key. */
  start: string;
  opened: number;
  merged: number;
}

export interface MergeStats {
  /** Median open→merge span in ms; null until something has merged. Median,
   *  not mean — one PR left open over a holiday would drag a mean into
   *  uselessness while the typical PR is unchanged. */
  medianMs: number | null;
  fastestMs: number | null;
  /** PRs with a merge time, lifetime. */
  merged: number;
  /** PRs still open right now. */
  open: number;
  /** Oldest-first weekly buckets. A week with no PRs is a real zero here —
   *  the log is complete, so absence is knowledge. */
  weeks: MergeWeek[];
}

const bump = (m: Map<string, number>, k: string) => m.set(k, (m.get(k) ?? 0) + 1);

function median(sorted: number[]): number | null {
  if (sorted.length === 0) return null;
  const mid = sorted.length >> 1;
  return sorted.length % 2 === 1 ? sorted[mid] : Math.round((sorted[mid - 1] + sorted[mid]) / 2);
}

export function mergeStats(rows: PrRow[], nowMs: number, weeks: number): MergeStats {
  const durations: number[] = [];
  const openedBy = new Map<string, number>();
  const mergedBy = new Map<string, number>();
  let merged = 0;
  let open = 0;

  for (const r of rows) {
    if (r.state === "open") open++;
    if (r.opened_at != null) bump(openedBy, weekStartDay(r.opened_at));
    if (r.merged_at != null) {
      merged++;
      bump(mergedBy, weekStartDay(r.merged_at));
      // A merge stamped at or before its open is a back-seeded or clock-skewed
      // row, not a zero-second PR. It still counts as merged — it just can't
      // contribute a duration.
      if (r.opened_at != null && r.merged_at > r.opened_at) {
        durations.push(r.merged_at - r.opened_at);
      }
    }
  }

  durations.sort((a, b) => a - b);
  return {
    medianMs: median(durations),
    fastestMs: durations[0] ?? null,
    merged,
    open,
    weeks: recentWeeks(nowMs, weeks).map((start) => ({
      start,
      opened: openedBy.get(start) ?? 0,
      merged: mergedBy.get(start) ?? 0,
    })),
  };
}

// ── spend over time ───────────────────────────────────────────────────────

/** One `usage_daily` row: a workspace's CUMULATIVE totals as of the last fold
 *  on that local day (see `recordUsageSnapshot`). */
export interface UsageRow {
  workspace_id: string;
  day: string;
  tokens: number;
  cost: number;
}

export interface SpendDay {
  day: string;
  /** Tokens attributable to this day, or `null` when no workspace produced a
   *  usable delta for it — unknown, not zero. */
  tokens: number | null;
  /** Same, in dollars. 0 is real: most providers report tokens but no cost. */
  cost: number | null;
}

/** Turn cumulative per-workspace snapshots into per-day project spend, over
 *  exactly the days in `days`.
 *
 *  Two things make this less obvious than a GROUP BY:
 *
 *  1. Rows are running totals, so a day's spend is the *difference* between
 *     consecutive rows for the same workspace.
 *  2. A workspace's FIRST row is therefore unattributable and is skipped. It
 *     is a total accumulated over an unknown stretch before it — `usage_daily`
 *     only starts recording once this tab has been opened — so plotting it as
 *     that day's spend would invent a launch-day spike that never happened.
 *     Undercounting the first observation is the honest failure here.
 *
 *  A day with no row at all for any workspace stays `null` rather than 0: the
 *  app simply never folded a session, which is not the same as a quiet day. */
export function dailySpend(rows: UsageRow[], days: string[]): SpendDay[] {
  const byWorkspace = new Map<string, UsageRow[]>();
  for (const r of rows) {
    const list = byWorkspace.get(r.workspace_id);
    if (list) list.push(r);
    else byWorkspace.set(r.workspace_id, [r]);
  }

  const observed = new Map<string, { tokens: number; cost: number }>();
  for (const list of byWorkspace.values()) {
    list.sort((a, b) => (a.day < b.day ? -1 : a.day > b.day ? 1 : 0));
    for (let i = 1; i < list.length; i++) {
      const cur = list[i];
      const prev = list[i - 1];
      const acc = observed.get(cur.day) ?? { tokens: 0, cost: 0 };
      // Clamped: a re-fold that shrinks a total (pruned records, a rebuilt
      // ledger) is a correction, never negative spend.
      acc.tokens += Math.max(0, cur.tokens - prev.tokens);
      acc.cost += Math.max(0, cur.cost - prev.cost);
      observed.set(cur.day, acc);
    }
  }

  return days.map((day) => {
    const hit = observed.get(day);
    return { day, tokens: hit?.tokens ?? null, cost: hit?.cost ?? null };
  });
}
