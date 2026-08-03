// An item's durable history, as the card renders it — the pure half of the
// lazy load, kept out of the hook so the ordering rules are testable without
// React.
//
// History is loaded per item on first expand, with the `roadmap:item-event`
// listener already live (the board subscribes before it fetches anything). That
// leaves one window: an event that lands while the snapshot request is in
// flight is delivered live *and* included in the snapshot. `mergeSnapshot`
// closes it the same way boardSync closes the board's — by id, so an event can
// appear once however it arrived.

import type { RoadmapEventKind, RoadmapItemEvent } from "@/api";

/** The kind, as the card's history line says it. */
export const EVENT_LABEL: Record<RoadmapEventKind, string> = {
  proposed: "Proposed",
  accepted: "Accepted",
  discarded: "Discarded",
  edited: "Edited",
  queued: "Queued",
  unqueued: "Taken off the queue",
  dispatched: "Dispatched",
  pr_opened: "PR opened",
  run_failed: "Run failed",
  shipped: "Shipped",
  abandoned: "Abandoned",
  blocked: "Blocked",
  note: "Note",
};

/** One history entry as one line: the kind, and the detail when there is one. */
export function eventLine(e: RoadmapItemEvent): string {
  return e.detail ? `${EVENT_LABEL[e.kind]} — ${e.detail}` : EVENT_LABEL[e.kind];
}

/** Newest first, ties kept in their existing relative order (the backend
 *  already breaks same-millisecond ties by write order). */
function newestFirst(list: RoadmapItemEvent[]): RoadmapItemEvent[] {
  return [...list].sort((a, b) => b.created_at - a.created_at);
}

/** Add one live event to a trail, newest first. A duplicate id is dropped — the
 *  same event can arrive live and in a snapshot merged moments later. */
export function insertEvent(list: RoadmapItemEvent[], e: RoadmapItemEvent): RoadmapItemEvent[] {
  if (list.some((x) => x.id === e.id)) return list;
  return newestFirst([e, ...list]);
}

/** Lay a fetched snapshot under whatever live events already arrived: dedupe by
 *  id (live wins, the rows are identical) and keep the whole trail newest
 *  first. */
export function mergeSnapshot(
  live: RoadmapItemEvent[],
  snapshot: RoadmapItemEvent[],
): RoadmapItemEvent[] {
  const seen = new Set(live.map((e) => e.id));
  return newestFirst([...live, ...snapshot.filter((e) => !seen.has(e.id))]);
}
