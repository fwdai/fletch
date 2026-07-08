import { localDay } from "@/util/format";

// Pure math behind the activity heatmap: grid construction, month labels,
// intensity levels, and the streak counter. Kept free of React/DOM so it's
// unit-testable and reusable by any surface that wants a contribution graph.

export interface HeatCell {
  /** Local date key, YYYY-MM-DD. */
  day: string;
  count: number;
  /** False for days after `endMs` (the tail of the current week) — rendered
   *  as invisible placeholders so the last column keeps its shape. */
  inRange: boolean;
}

const DAY_MS = 86_400_000;

/** Monday-first weekday index (Mon=0 … Sun=6). */
const mondayIdx = (d: Date) => (d.getDay() + 6) % 7;

/** Noon of the given day — stepping whole days from noon never crosses a DST
 *  boundary into the wrong date. */
const atNoon = (ms: number): number => {
  const d = new Date(ms);
  d.setHours(12, 0, 0, 0);
  return d.getTime();
};

/** Build a Monday-first week grid ending at the week containing `endMs`:
 *  `weeks` columns × 7 rows, oldest week first. */
export function buildHeatmapWeeks(
  endMs: number,
  weeks: number,
  counts: Record<string, number>,
): HeatCell[][] {
  const end = atNoon(endMs);
  const start = end - (mondayIdx(new Date(end)) + (weeks - 1) * 7) * DAY_MS;
  const grid: HeatCell[][] = [];
  for (let w = 0; w < weeks; w++) {
    const col: HeatCell[] = [];
    for (let d = 0; d < 7; d++) {
      const t = start + (w * 7 + d) * DAY_MS;
      const day = localDay(t);
      col.push({ day, count: counts[day] ?? 0, inRange: t <= end });
    }
    grid.push(col);
  }
  return grid;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** One label slot per week column: the month name where a column starts a new
 *  month, null elsewhere. The first column is only labeled when it starts at
 *  a month boundary, so a mid-month left edge isn't mislabeled. */
export function monthLabels(grid: HeatCell[][]): (string | null)[] {
  let prev = -1;
  return grid.map((col, i) => {
    const month = Number(col[0].day.slice(5, 7)) - 1;
    const label = i === 0 ? Number(col[0].day.slice(8, 10)) <= 7 : month !== prev;
    prev = month;
    return label ? (MONTHS[month] ?? null) : null;
  });
}

/** Map a count onto the 0–4 intensity scale, scaled to the busiest day. */
export function heatLevel(count: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0 || max <= 0) return 0;
  return Math.min(4, Math.max(1, Math.ceil((count / max) * 4))) as 1 | 2 | 3 | 4;
}

/** Consecutive active days ending today. Today with no activity yet doesn't
 *  break the streak (the day isn't over) — it just doesn't count. */
export function computeStreak(counts: Record<string, number>, todayMs: number): number {
  let t = atNoon(todayMs);
  if (!counts[localDay(t)]) t -= DAY_MS;
  let streak = 0;
  while ((counts[localDay(t)] ?? 0) > 0) {
    streak++;
    t -= DAY_MS;
  }
  return streak;
}

/** "Tue, Jul 8" for a YYYY-MM-DD key, in the user's locale. */
export function formatHeatDay(day: string): string {
  const d = new Date(
    Number(day.slice(0, 4)),
    Number(day.slice(5, 7)) - 1,
    Number(day.slice(8, 10)),
  );
  return d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}
