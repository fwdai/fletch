// The roadmap board's rows, as a subscription — for every surface that needs to
// know what is on a board without owning one.
//
// Lives here rather than under `components/ProjectScreen/Roadmap/` because it has
// three consumers in three different areas: the board itself (`useRoadmap`), the
// workspace's issue inbox (`MissionControl/useIssueFunnel`), and a run's monitor
// (`RunView/RoadmapChip`). The same promotion `rowSync.ts` got when its third
// caller appeared, and for the same reason — a shared hook whose home is one
// caller's folder makes the other two import *through* a screen they have nothing
// to do with.
//
// Every surface here is event-driven, not polled. The funnel used to poll
// (`roadmap_list_items` per project, every two minutes), which bought up to two
// minutes of disagreement with the board: discarding a routed ghost left the
// inbox saying "On roadmap", and the click-dedup could only see ghosts this
// window had created. The chip fetched once per mount and never subscribed, so an
// accepted retitle left a stale title on screen and a shipped item kept a chip
// the board would silently swallow the click of. Both are the same defect —
// a surface beside the spine rather than on it — and both are fixed by riding
// `roadmap:item` / `roadmap:item-deleted` like the board does.
//
// The load discipline is `rowSync`'s: subscribe, then fetch, then replay what
// arrived in between (see rowSync.ts for the two loss windows that buys). Worth
// the ceremony for exactly the reason the board needs it — these rows have
// writers the user isn't (the PM's propose, the queue's claim, the sweep's ship).

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, onRoadmapItem, onRoadmapItemDeleted, type RoadmapItem } from "@/api";
import { applyRowEvent, createRowSync } from "@/rowSync";

/** How one project's board is doing.
 *
 *  `failed` is not `loaded` with no rows, and callers must not fold the two: the
 *  funnel's whole dedup rests on "what is already routed onto this board?", and a
 *  read that failed answering "nothing" would re-enable Add on every routed row
 *  and let one click stack a second ghost. A project absent from the map has not
 *  been read yet, which is the same unknown. */
export type BoardLoad = "loading" | "loaded" | "failed";

export interface RoadmapRows {
  /** Every listed project's rows, flat — `project_id` is on the row, so callers
   *  that want them grouped group them. Includes `done` rows: what a surface
   *  hides is its own business. */
  rows: readonly RoadmapItem[];
  /** Per project, how its snapshot went. */
  load: ReadonlyMap<string, BoardLoad>;
  /** Any listed project still waiting on its snapshot. False when nothing is
   *  listed — "no boards" is a settled answer, not a pending one. */
  loading: boolean;
  /** The first snapshot failure, for a caller with an error bar. Null while every
   *  listed board is loading or loaded. */
  error: string | null;
  /** Fold a row a command just returned straight in, without waiting for its
   *  event — the optimistic half of a mutation the caller already awaited. */
  upsert: (row: RoadmapItem) => void;
  /** Drop rows a command just deleted, same deal. */
  remove: (ids: readonly string[]) => void;
}

/** Side channels for a caller that needs to act on an *arrival* rather than on
 *  the resulting state — a distinction React state can't express, because two
 *  events in one frame collapse into one render. */
export interface RoadmapRowsOptions {
  /** A row arrived on `roadmap:item` (not a snapshot row, and not an optimistic
   *  upsert). The board uses it to drop the stale queue note a moving row
   *  invalidates. */
  onRow?: (row: RoadmapItem) => void;
  /** A row was deleted on `roadmap:item-deleted`. The board uses it to drop the
   *  trail it was holding; the funnel uses it to remember a refusal. */
  onDeleted?: (id: string) => void;
}

/** Follow the rows of one or more project boards, live.
 *
 *  `projectIds` may change freely — the effect keys on the *contents*, so a
 *  caller need not memoize the array (`[a, b]` rebuilt every render resubscribes
 *  nothing). An empty list holds no rows, no subscription, and no pending load.
 *
 *  Callbacks are read through a ref, so a caller can pass fresh closures without
 *  tearing the subscription down. */
export function useRoadmapRows(
  projectIds: readonly string[],
  options: RoadmapRowsOptions = {},
): RoadmapRows {
  const [rows, setRows] = useState<RoadmapItem[]>([]);
  const [load, setLoad] = useState<ReadonlyMap<string, BoardLoad>>(() => new Map());
  const [error, setError] = useState<string | null>(null);
  // Identity-independent: the same ids in the same order are the same
  // subscription, however the caller built the array.
  const key = projectIds.join(" ");
  const handlers = useRef(options);
  handlers.current = options;

  useEffect(() => {
    const ids = key ? key.split(" ") : [];
    if (ids.length === 0) {
      setRows([]);
      setLoad(new Map());
      setError(null);
      return;
    }
    let alive = true;
    setRows([]);
    setError(null);
    setLoad(new Map(ids.map((id) => [id, "loading" as BoardLoad])));

    const wanted = new Set(ids);
    // One sequencer for all the listed boards: the rows are one flat list keyed
    // by row id, and a project's snapshot replaying over it only ever adds its
    // own rows (see `settle` below).
    const sync = createRowSync<RoadmapItem>((update) => {
      if (alive) setRows(update);
    });
    const off = onRoadmapItem((row) => {
      if (!wanted.has(row.project_id)) return;
      sync.push({ kind: "upsert", row });
      handlers.current.onRow?.(row);
    });
    // Addressed by item id and carries no project, so it can't be filtered:
    // a delete for a row no listed board holds removes nothing, which is the
    // right answer anyway.
    const offDeleted = onRoadmapItemDeleted((id) => {
      sync.push({ kind: "delete", id });
      handlers.current.onDeleted?.(id);
    });

    void (async () => {
      // Awaited, not merely started: Tauri's `listen` resolves asynchronously,
      // and an event emitted before it does is never delivered at all.
      await Promise.all([off, offDeleted]);
      if (!alive) return;
      const snapshots = await Promise.all(
        ids.map(async (id): Promise<Snapshot> => {
          try {
            return { id, rows: await api.roadmapListItems(id) };
          } catch (e) {
            return { id, failure: String(e) };
          }
        }),
      );
      if (!alive) return;
      // One `settle` for the whole set, so the buffered live events replay
      // exactly once and over every snapshot rather than per project.
      sync.settle(snapshots.flatMap((s) => ("rows" in s ? s.rows : [])));
      setLoad(new Map(snapshots.map((s) => [s.id, "rows" in s ? "loaded" : "failed"])));
      setError(snapshots.find((s): s is Failed => "failure" in s)?.failure ?? null);
    })();

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, [key]);

  const upsert = useCallback((row: RoadmapItem) => {
    setRows((prev) => applyRowEvent(prev, { kind: "upsert", row }));
  }, []);
  const remove = useCallback((ids: readonly string[]) => {
    setRows((prev) => prev.filter((r) => !ids.includes(r.id)));
  }, []);

  const loading = useMemo(() => [...load.values()].some((s) => s === "loading"), [load]);
  return { rows, load, loading, error, upsert, remove };
}

/** One project's snapshot attempt — read, or a reason it wasn't. */
type Snapshot = { id: string; rows: RoadmapItem[] } | Failed;
type Failed = { id: string; failure: string };

/** One row by id, live — for a surface that holds an item id but no board.
 *
 *  A run's monitor is the case: it has `wf_run.roadmap_item_id` and nothing else,
 *  and the run outlives the screen the item was queued from. Subscribed rather
 *  than fetched once, because everything the chip renders can change under it: an
 *  accepted PM retitle changes the title, and the merge sweep shipping the item
 *  changes where (or whether) the chip can go.
 *
 *  `null` means "no such row" *or* "not read yet" — the two are the same to a
 *  caller that renders nothing either way, and a dead link is worse than no
 *  link. */
export function useRoadmapRow(itemId: string | null): RoadmapItem | null {
  const [row, setRow] = useState<RoadmapItem | null>(null);

  useEffect(() => {
    setRow(null);
    if (!itemId) return;
    let alive = true;
    // "One row or none, keyed by something we already know" is `createSingleSync`
    // territory, but this stream is keyed by *item id* and the row carries it, so
    // the filter is exact and the merge is the same last-write-wins. Written out
    // rather than reused because there is no snapshot-vs-live conflict to
    // resolve: the fetch below is the oldest possible value.
    let live = false;
    const off = onRoadmapItem((r) => {
      if (!alive || r.id !== itemId) return;
      live = true;
      setRow(r);
    });
    const offDeleted = onRoadmapItemDeleted((id) => {
      if (!alive || id !== itemId) return;
      live = true;
      setRow(null);
    });

    void (async () => {
      await Promise.all([off, offDeleted]);
      if (!alive) return;
      try {
        const fetched = await api.roadmapGetItem(itemId);
        // Anything that arrived live is newer than the snapshot by construction.
        if (alive && !live) setRow(fetched);
      } catch {
        // A convenience surface; a failed read leaves the caller with nothing to
        // render, which is what it renders when the row is gone too.
      }
    })();

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, [itemId]);

  return row;
}
