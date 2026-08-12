// Where a status puts a row on this surface — the one partition the horizon
// groups, the header counts and the "Not doing" section all read.
//
// Pure and in its own file (like ruling.ts) because three different numbers are
// derived from it — the group counts, the header's shipped count, the decision
// log's length — and a predicate inlined in the hook is exactly how one of them
// drifts: when `rejected` arrived, every `status !== "done"` in the hook would
// have silently kept counting rejected rows as open work.

import type { RoadmapItem } from "@/api";

/** Shipped. The item leaves the board entirely and survives only as the
 *  header's count (and the Activity tab's record). */
export const isShipped = (i: RoadmapItem) => i.status === "done";

/** Ruled off the board, with the reason kept (`close_reason`). Not deleted:
 *  the row and its history survive in the board's collapsed "Not doing"
 *  section, from which it can be reopened. In no horizon group and no count —
 *  a rejected item is not open work. */
export const isRejected = (i: RoadmapItem) => i.status === "rejected";

/** On the board proper — in a horizon group, counted, joinable by the strips.
 *  Everything but the two departures above: `done` left forward, `rejected`
 *  left sideways. */
export const isOnBoard = (i: RoadmapItem) => !isShipped(i) && !isRejected(i);

/** A row the PM has suggested and the user hasn't ruled on. Drawn as a ghost:
 *  in its target horizon, but counted for nothing. */
export const isProposed = (i: RoadmapItem) => i.status === "proposed";

/** Can this row's position in the order be changed — by a drag, or by an
 *  accepted PM reordering? Everything from `active` on has been dispatched, so
 *  its place in the queue is settled and moving it would mean nothing (the same
 *  three statuses the backend's `order::is_orderable` allows). */
export const isOrderable = (i: RoadmapItem) =>
  i.status === "proposed" || i.status === "open" || i.status === "queued";

/** The decision log, newest ruling first. `updated_at` rather than a dedicated
 *  timestamp: rejecting is the row's last write (nothing else touches a
 *  rejected item until a reopen removes it from this list), so the two agree. */
export function rejectedRows(rows: readonly RoadmapItem[]): RoadmapItem[] {
  return rows.filter(isRejected).sort((a, b) => b.updated_at - a.updated_at);
}
