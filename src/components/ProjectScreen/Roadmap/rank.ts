// Where a dragged card lands in the priority order — pure over the destination
// group's rows and the drop index, so the arithmetic is testable without a DOM,
// a drag event, or a backend.
//
// The order is stored as `roadmap_items.rank` (a REAL; see migration 0032) and
// maintained by *fractional indexing*: dropping a card between two neighbours
// stores the midpoint of their ranks, so one row is written and nothing else on
// the board moves. Appending past either end steps a whole 1 beyond it, which
// keeps the usual case (drag to the top, drag to the bottom) at integer-ish
// values rather than halving a gap forever.
//
// The degenerate case is a gap with nothing left in it: two rows that already
// share a rank, or a gap halved so many times that the midpoint equals one of
// its ends (~50 drops between the same two neighbours). Fractional indexing
// cannot express a position there, so the plan falls back to *renumbering the
// destination group* — one sequential write per row, starting from the group's
// own lowest rank so the group stays where it was relative to the rest of the
// board. Unreachable in practice; deterministic when reached, which is the
// point of computing it here instead of hoping.

/** The rank arithmetic only needs an id and a rank; the callers pass rows. */
export interface Ranked {
  id: string;
  rank: number;
}

/** How far beyond the edge an append or prepend lands. */
export const RANK_STEP = 1;

/** What to write for a drop. `set` is the fractional-index case — one rank for
 *  the dragged row. `renumber` is the fallback: every row in the destination
 *  group gets a fresh rank, the dragged one included (it is in `writes`, at its
 *  new index). */
export type RankPlan = { kind: "set"; rank: number } | { kind: "renumber"; writes: Ranked[] };

/** The rank between two neighbours, or `null` when the gap holds nothing — the
 *  neighbours are equal, or the midpoint has collapsed onto an end. `undefined`
 *  for a neighbour means "the edge of the list". */
export function rankBetween(above: number | undefined, below: number | undefined): number | null {
  if (above === undefined && below === undefined) return RANK_STEP;
  if (above === undefined) return (below as number) - RANK_STEP;
  if (below === undefined) return above + RANK_STEP;
  if (!(above < below)) return null;
  const mid = (above + below) / 2;
  return mid > above && mid < below ? mid : null;
}

/** Plan the write(s) for dropping `movedId` at `index` in a group.
 *
 *  `rows` is the destination group in display order **without** the dragged row,
 *  and `index` is the position it should end up at (0 = first, `rows.length` =
 *  last). Out-of-range indices are clamped, so a drop on a row that vanished
 *  mid-drag still lands somewhere sensible. */
export function planDrop(rows: Ranked[], index: number, movedId: string): RankPlan {
  const at = Math.max(0, Math.min(index, rows.length));
  const rank = rankBetween(rows[at - 1]?.rank, rows[at]?.rank);
  if (rank !== null) return { kind: "set", rank };

  // No room between the neighbours: rewrite the whole group instead, keeping it
  // in the same region of the board's sequence.
  const start = Math.max(RANK_STEP, Math.floor(Math.min(...rows.map((r) => r.rank))));
  const final = [...rows.slice(0, at), { id: movedId, rank: 0 }, ...rows.slice(at)];
  return {
    kind: "renumber",
    writes: final.map((r, i) => ({ id: r.id, rank: start + i * RANK_STEP })),
  };
}

/** Where a drop on a card inserts, given the group's rows without the dragged
 *  row: `before` the target, or after it. Returns the index [`planDrop`] wants.
 *  A target that is no longer in the list appends. */
export function dropIndex(rows: Ranked[], targetId: string, edge: "before" | "after"): number {
  const i = rows.findIndex((r) => r.id === targetId);
  if (i < 0) return rows.length;
  return edge === "before" ? i : i + 1;
}
