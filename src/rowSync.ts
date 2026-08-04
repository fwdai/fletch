// The load ordering for any list the backend both snapshots and streams, kept
// out of the hooks so it can be reasoned about (and tested) without React.
//
// Loading is subscribe-first, not fetch-first. Tauri's `listen` resolves
// asynchronously, so a fetch-first list has two windows where a row written by
// someone else is lost: an event emitted before registration resolves is never
// delivered, and an event that lands after the backend read its snapshot but
// before the fetch's `.then` runs is clobbered by the wholesale `setRows(snapshot)`.
//
// `createRowSync` closes both: events that arrive while the snapshot is in
// flight go into one ordered buffer (upserts and deletes together, so their
// relative order survives) and are replayed over the snapshot the moment it
// lands. After that every event applies straight through.
//
// Generic over the row, because the same races exist for every stream the app
// follows and each one loses something different:
//   - the roadmap board (`roadmap:item`) — the PM's `roadmap_propose`, and later
//     the run queue's claim;
//   - the PM's pending proposals (`roadmap:proposal`, whose replacements arrive
//     as upserts under a stable id);
//   - the run list (`wf:run`) — a *pause*, which has no follow-up event to
//     repair it: a run that stops for a human question emits once and then waits,
//     so a clobbered pause stays invisible until something else moves.
//
// `createSingleSync` is the same discipline for a stream that is one row or none
// (the PM's whole-board order ask; the board's hold), where "last write wins" is
// the whole merge.

/** A row change from the backend, tagged so upserts and deletes can share one
 *  ordered buffer. */
export type RowEvent<Row extends { id: string }> =
  | { kind: "upsert"; row: Row }
  | { kind: "delete"; id: string };

/** Apply one event to a row list. Upsert replaces by id and otherwise appends —
 *  the backend lists oldest-first and a new row is the newest, so appending keeps
 *  the two in the same order. */
export function applyRowEvent<Row extends { id: string }>(rows: Row[], e: RowEvent<Row>): Row[] {
  if (e.kind === "delete") return rows.filter((r) => r.id !== e.id);
  return rows.some((r) => r.id === e.row.id)
    ? rows.map((r) => (r.id === e.row.id ? e.row : r))
    : [...rows, e.row];
}

export interface RowSync<Row extends { id: string }> {
  /** Feed in an event. Buffered until `settle`, applied directly after it. */
  push(e: RowEvent<Row>): void;
  /** The initial load finished: replay everything buffered over `snapshot` (or,
   *  when the fetch failed and there is no snapshot, over the rows already held)
   *  and stop buffering. Idempotent — a second call just applies nothing. */
  settle(snapshot?: Row[]): void;
}

/** Buffer-then-replay sequencer for one list load.
 *
 *  `commit` is the state writer — a React `setRows`-shaped updater call. It is
 *  invoked at most once per event, and exactly once by `settle`. */
export function createRowSync<Row extends { id: string }>(
  commit: (update: (prev: Row[]) => Row[]) => void,
): RowSync<Row> {
  /** Non-null only while the snapshot is in flight. */
  let buffered: RowEvent<Row>[] | null = [];
  return {
    push(e) {
      if (buffered) buffered.push(e);
      else commit((prev) => applyRowEvent(prev, e));
    },
    settle(snapshot) {
      const pending = buffered ?? [];
      buffered = null;
      commit((prev) => pending.reduce(applyRowEvent, snapshot ?? prev));
    },
  };
}

export interface SingleSync<Row> {
  /** Feed in the current value (`null` = gone). Buffered until `settle`. */
  push(value: Row | null): void;
  /** The initial load finished: whatever arrived live wins over `fetched`, and
   *  buffering stops. Called with no argument when the fetch failed, which leaves
   *  the held value alone unless something did arrive live. */
  settle(fetched?: Row | null): void;
}

/** [`createRowSync`] for a stream that is *one row, or none*, keyed by something
 *  the board already knows (a project id) rather than by a row id.
 *
 *  Same two loss windows, so the same subscribe-then-fetch-then-replay
 *  discipline — collapsed to "last write wins", because there is no list to
 *  merge into and no relative order to preserve: the newest value simply *is* the
 *  state. Used for the PM's whole-board order ask and for the board's hold, both
 *  of which the backend replaces in place and addresses by project.
 *
 *  Note the deliberate distinction between `undefined` ("nothing arrived") and
 *  `null` ("it was released"): folding them together would let a stale snapshot
 *  resurrect a hold the user lifted while it was in flight. */
export function createSingleSync<Row>(commit: (value: Row | null) => void): SingleSync<Row> {
  let settled = false;
  let buffered: Row | null | undefined;
  return {
    push(value) {
      if (settled) commit(value);
      else buffered = value;
    },
    settle(fetched) {
      settled = true;
      if (buffered !== undefined) commit(buffered);
      else if (fetched !== undefined) commit(fetched);
    },
  };
}
