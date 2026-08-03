// The board's load ordering, kept out of the hook so it can be reasoned about
// (and tested) without React.
//
// Loading is subscribe-first, not fetch-first. Tauri's `listen` resolves
// asynchronously, so a fetch-first board has two windows where a row written by
// someone else — the PM's `roadmap_propose`, and later the run queue — is lost:
// an event emitted before registration resolves is never delivered, and an event
// that lands after the backend read its snapshot but before the fetch's `.then`
// runs is clobbered by the wholesale `setRows(snapshot)`.
//
// `createBoardSync` closes both: events that arrive while the snapshot is in
// flight go into one ordered buffer (upserts and deletes together, so their
// relative order survives) and are replayed over the snapshot the moment it
// lands. After that every event applies straight through.
//
// Generic over the row: the same races exist for every stream the board
// follows, so the PM's pending proposals (`roadmap:proposal`, whose
// replacements arrive as upserts under a stable id) ride a second instance of
// this sequencer rather than a re-derivation of it.

import type { RoadmapItem } from "@/api";

/** A row change from the backend, tagged so upserts and deletes can share one
 *  ordered buffer. */
export type BoardEvent<Row extends { id: string } = RoadmapItem> =
  | { kind: "upsert"; row: Row }
  | { kind: "delete"; id: string };

/** Apply one event to a row list. Upsert replaces by id and otherwise appends —
 *  the backend lists oldest-first and a new row is the newest, so appending keeps
 *  the two in the same order. */
export function applyBoardEvent<Row extends { id: string }>(
  rows: Row[],
  e: BoardEvent<Row>,
): Row[] {
  if (e.kind === "delete") return rows.filter((r) => r.id !== e.id);
  return rows.some((r) => r.id === e.row.id)
    ? rows.map((r) => (r.id === e.row.id ? e.row : r))
    : [...rows, e.row];
}

export interface BoardSync<Row extends { id: string } = RoadmapItem> {
  /** Feed in an event. Buffered until `settle`, applied directly after it. */
  push(e: BoardEvent<Row>): void;
  /** The initial load finished: replay everything buffered over `snapshot` (or,
   *  when the fetch failed and there is no snapshot, over the rows already held)
   *  and stop buffering. Idempotent — a second call just applies nothing. */
  settle(snapshot?: Row[]): void;
}

/** Buffer-then-replay sequencer for one board load.
 *
 *  `commit` is the state writer — a React `setRows`-shaped updater call. It is
 *  invoked at most once per event, and exactly once by `settle`. */
export function createBoardSync<Row extends { id: string } = RoadmapItem>(
  commit: (update: (prev: Row[]) => Row[]) => void,
): BoardSync<Row> {
  /** Non-null only while the snapshot is in flight. */
  let buffered: BoardEvent<Row>[] | null = [];
  return {
    push(e) {
      if (buffered) buffered.push(e);
      else commit((prev) => applyBoardEvent(prev, e));
    },
    settle(snapshot) {
      const pending = buffered ?? [];
      buffered = null;
      commit((prev) => pending.reduce(applyBoardEvent, snapshot ?? prev));
    },
  };
}
