// The Roadmap board's single source of truth: the project's items, and the
// mutations every surface on this screen goes through.
//
// The board is persisted, per project, in `roadmap_items` (src-tauri/src/roadmap).
// This hook loads it once for the current project and then keeps it live off the
// `roadmap:item` / `roadmap:item-deleted` events, upserting the full row by id.
// The load subscribes before it fetches and replays anything that arrived in
// between (see boardSync.ts), because this board has writers other than the user.
//
// The PM conversation is NOT here: it is a real agent chat, owned by the Thread
// column (see Thread/usePmChats.ts). What the two share is this contract — the
// PM proposes, the user commits, nothing reaches the board unaccepted.
//
// A proposal is not a client-side draft: `roadmap_propose` (the PM's RPC tool,
// src-tauri/src/rpc/roadmap.rs) writes real rows with `status: "proposed"`, and
// they arrive here on the same `roadmap:item` event as everything else — so the
// board grows ghost rows live while the PM is still talking. Accepting one is a
// status patch (`proposed → open`); discarding it is a delete. Those two are
// the only ways a proposed row leaves that state.
//
// Queueing works the same way, one status further along: `open → queued` is the
// user handing an item to the Rust drainer (src-tauri/src/roadmap/drainer.rs),
// which owns everything after it — `queued → active` when it launches a run,
// and `active → in_review`/`done`/back to `open` when that run settles. This
// hook never writes those; it only ever asks for `queued` or takes it back.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type NewRoadmapItem,
  onRoadmapItem,
  onRoadmapItemDeleted,
  onRoadmapQueueNote,
  type RoadmapItem,
  type RoadmapItemPatch,
} from "@/api";
import { useAppStore } from "@/store";
import { applyBoardEvent, createBoardSync } from "./boardSync";
import { PRODUCT_MAP } from "./mockData";
import type { Horizon } from "./types";
import { toBoardItem } from "./types";
import { useProjectWorkflows } from "./useProjectWorkflows";

/** How long a row stays highlighted after landing on the board. */
const LANDED_MS = 2200;
/** How long a focused row keeps its ring after being jumped to. */
const FOCUS_MS = 2200;

export type BoardTab = "roadmap" | "map";

/** A shipped item leaves the board entirely and survives only as the header's
 *  count, so "on the board" is every status but `done`. */
const isOnBoard = (i: RoadmapItem) => i.status !== "done";

/** A row the PM has suggested and the user hasn't ruled on. Drawn as a ghost:
 *  in its target horizon, but counted for nothing. */
const isProposed = (i: RoadmapItem) => i.status === "proposed";

export function useRoadmap(repoPath: string) {
  // The board is per project, not per repo: a multi-repo project has one
  // roadmap. Resolved from the pinned-repo list the sidebar already loads.
  const projectId =
    useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id) ?? null;
  // Until the workspace itself is loaded, a missing project_id means "not known
  // yet", not "no project" — telling a populated board it's empty for a frame
  // would flash the empty state at someone who has a roadmap.
  const workspaceReady = useAppStore((s) => s.workspace != null);
  // What a queued item will run under. Loaded here rather than in the Board so
  // the item form and the card's queue affordance read the same answer.
  const workflows = useProjectWorkflows(projectId);

  const [rows, setRows] = useState<RoadmapItem[]>([]);
  const [loading, setLoading] = useState(true);
  /** The last failure from a mutation with no form of its own to report into
   *  (a move, a delete, an accepted proposal). */
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<BoardTab>("roadmap");
  const [openCodes, setOpenCodes] = useState<ReadonlySet<string>>(() => new Set());
  const [focusCode, setFocusCode] = useState<string | null>(null);
  /** Codes highlighted because they just landed or just moved. */
  const [landed, setLanded] = useState<ReadonlySet<string>>(() => new Set());
  /** Why a queued item isn't moving, by item id — the drainer's transient
   *  `roadmap:queue-note`. Not persisted anywhere, on purpose: it's only true
   *  until the next tick, and the row's own next change supersedes it. */
  const [notes, setNotes] = useState<ReadonlyMap<string, string>>(() => new Map());

  // Every pending highlight timer, so unmounting can't set state on a dead
  // component.
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  useEffect(() => {
    const pending = timers.current;
    return () => {
      for (const t of pending) clearTimeout(t);
    };
  }, []);
  const after = useCallback((ms: number, fn: () => void) => {
    timers.current.push(setTimeout(fn, ms));
  }, []);

  // ── the persisted board ────────────────────────────────────────────
  /** Drop an item's queue note. Notes are independent of the row buffer, so
   *  clearing at event-arrival time is safe even mid-load. */
  const dropNote = useCallback((id: string) => {
    setNotes((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Map(prev);
      next.delete(id);
      return next;
    });
  }, []);

  /** Upsert a row by id, appending new ones — the backend lists oldest-first
   *  and a new row is the newest, so append keeps the two in the same order. */
  const upsert = useCallback(
    (row: RoadmapItem) => {
      setRows((prev) => applyBoardEvent(prev, { kind: "upsert", row }));
      // A note explains why a *queued* row is stuck. The moment the row moves
      // on, whatever it said is history — drop it rather than leave a stale
      // excuse under a running item.
      if (row.status !== "queued") dropNote(row.id);
    },
    [dropNote],
  );

  // Load the board: subscribe first, buffer during the fetch, then replay — so a
  // row the PM proposes while the board is loading cannot be lost. Rows change
  // from more than this screen (the PM agent's own writes, and later the run
  // queue), so the board follows the event rather than only its own command
  // results; the ordering the sequencer buys us is spelled out in boardSync.ts.
  useEffect(() => {
    if (!projectId) {
      setRows([]);
      setLoading(!workspaceReady);
      return;
    }
    let alive = true;
    setLoading(true);

    // Unmounting mid-load must not write state, so every commit goes through
    // the same `alive` gate the fetch does.
    const sync = createBoardSync((update) => {
      if (alive) setRows(update);
    });
    const off = onRoadmapItem((row) => {
      if (row.project_id !== projectId) return;
      sync.push({ kind: "upsert", row });
      // Rows go through the sequencer; notes don't need to — they're a
      // separate map, and a stale excuse should vanish even mid-load.
      if (row.status !== "queued") dropNote(row.id);
    });
    const offDeleted = onRoadmapItemDeleted((id) => {
      sync.push({ kind: "delete", id });
      dropNote(id);
    });
    // Addressed by item id, and every id on this board belongs to this project,
    // so no extra filtering is needed — a note for a row we don't hold is
    // simply never rendered.
    const offNote = onRoadmapQueueNote(({ item_id, note }) => {
      setNotes((prev) => new Map(prev).set(item_id, note));
    });

    void (async () => {
      // Registration has to be awaited, not just started: an event emitted
      // before `listen` resolves never reaches us at all.
      await Promise.all([off, offDeleted]);
      if (!alive) return;
      try {
        const items = await api.roadmapListItems(projectId);
        if (!alive) return;
        sync.settle(items);
        setLoading(false);
      } catch (e) {
        if (!alive) return;
        // No snapshot to replay over — settle anyway so later events still
        // apply instead of piling up in the buffer.
        sync.settle();
        setError(String(e));
        setLoading(false);
      }
    })();

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
      void offNote.then((f) => f());
    };
  }, [projectId, workspaceReady, dropNote]);

  /** Light up rows for a moment after they land, then clear only those — a
   *  later landing must not have its highlight cut short by an earlier timer. */
  const markLanded = useCallback(
    (codes: string[]) => {
      if (!codes.length) return;
      setLanded((prev) => new Set([...prev, ...codes]));
      after(LANDED_MS, () =>
        setLanded((prev) => {
          const next = new Set(prev);
          for (const c of codes) next.delete(c);
          return next;
        }),
      );
    },
    [after],
  );

  // ── derived ────────────────────────────────────────────────────────
  const items = useMemo(
    () => rows.filter((r) => isOnBoard(r) && !isProposed(r)).map(toBoardItem),
    [rows],
  );
  /** Shipped items aren't on the board; the header carries the count. */
  const shipped = useMemo(() => rows.filter((r) => !isOnBoard(r)).length, [rows]);

  /** The PM's outstanding proposals, drawn on the board as ghosts until the
   *  user accepts or discards them. Kept out of `items` so they don't move a
   *  single count before that. */
  const ghosts = useMemo(() => rows.filter(isProposed).map(toBoardItem), [rows]);

  const counts = useMemo(() => {
    const by: Record<Horizon, number> = { now: 0, next: 0, later: 0 };
    for (const i of items) by[i.horizon] += 1;
    return by;
  }, [items]);

  // ── board mutations ────────────────────────────────────────────────
  /** Add a row; the code is allocated backend-side. Throws on failure, because
   *  its caller is a form that can say so inline. */
  const addItem = useCallback(
    async (input: NewRoadmapItem) => {
      if (!projectId) throw new Error("This repo isn't part of a project yet.");
      const row = await api.roadmapCreateItem(projectId, input);
      upsert(row);
      markLanded([row.code]);
      return row;
    },
    [projectId, upsert, markLanded],
  );

  /** Edit a row. Throws, like `addItem` — its caller is the same form. */
  const editItem = useCallback(
    async (id: string, patch: RoadmapItemPatch) => {
      const { item } = await api.roadmapUpdateItem(id, patch);
      upsert(item);
      return item;
    },
    [upsert],
  );

  /** Run a mutation whose caller has nowhere to render a failure, and surface
   *  it on the board instead of dropping it on the floor. */
  const guarded = useCallback(async (fn: () => Promise<unknown>) => {
    try {
      setError(null);
      await fn();
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const moveItem = useCallback(
    (id: string, to: Horizon) =>
      guarded(async () => {
        const { item } = await api.roadmapUpdateItem(id, { horizon: to });
        upsert(item);
        markLanded([item.code]);
      }),
    [guarded, markLanded, upsert],
  );

  /** Delete rows. Also how a proposal is discarded: a suggestion the user turned
   *  down was never on the roadmap, so it leaves no trace of having been. */
  const removeItems = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        for (const id of ids) await api.roadmapDeleteItem(id);
        setRows((prev) => prev.filter((r) => !ids.includes(r.id)));
      }),
    [guarded],
  );

  const clearError = useCallback(() => setError(null), []);

  /** Accept proposed rows: `proposed → open` is the moment a suggestion becomes
   *  a roadmap item. The row (and its code) already exist, so nothing is
   *  re-created and nothing is renumbered — the code the PM quoted in the chat
   *  is the code that stays on the board. The other half of the decision is
   *  [`removeItems`]. */
  const acceptItems = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        const codes: string[] = [];
        for (const id of ids) {
          const { item } = await api.roadmapUpdateItem(id, { status: "open" });
          upsert(item);
          codes.push(item.code);
        }
        markLanded(codes);
      }),
    [guarded, markLanded, upsert],
  );

  /** Hand items to the drainer: `open → queued`. From here the Rust side owns
   *  the item — it picks the oldest queued item whose dependencies have landed,
   *  resolves a workflow, and launches a run. Queueing something it can't run
   *  yet is fine and deliberate: it waits, and says why on the card.
   *
   *  Conditional on `open` for symmetry with [`unqueueItems`]: a row that already
   *  moved on (the PM re-proposed it, another window queued it) is reported as it
   *  is rather than dragged back to `queued`. */
  const queueItems = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        for (const id of ids) {
          // Any note this row carries is about its previous life ("Back on the
          // board — its run failed."), not about the queued one, and the `queued`
          // row coming back won't clear it — `upsert` only drops notes for rows
          // that left the queue. Dropped *before* the write, so the note the
          // drainer emits on the resulting nudge is the one left standing.
          dropNote(id);
          const { item } = await api.roadmapUpdateItem(id, { status: "queued" }, "open");
          upsert(item);
        }
      }),
    [dropNote, guarded, upsert],
  );

  /** Take an item back off the queue before it's dispatched (`queued → open`).
   *
   *  Conditional on `queued`, because racing the drainer is *not* safe: it claims
   *  `queued → active` under the connection lock and only then writes the
   *  launched run's id onto the row, so a blind `→ open` landing in between would
   *  leave a live run tied to an item nothing ever settles (the drainer settles
   *  `active` items only) — the run would finish invisibly while holding the
   *  project's queue slot, and the item would sit `open` forever.
   *
   *  A miss means the drainer got there first. That is not an error to shout
   *  about: the click was simply a moment late, so the row that comes back
   *  (`active`) is upserted and the board draws it as running, because it is. */
  const unqueueItems = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        for (const id of ids) {
          const { item } = await api.roadmapUpdateItem(id, { status: "open" }, "queued");
          upsert(item);
        }
      }),
    [guarded, upsert],
  );

  // ── board interaction ──────────────────────────────────────────────
  const toggleItem = useCallback((code: string) => {
    setOpenCodes((s) => {
      const next = new Set(s);
      if (!next.delete(code)) next.add(code);
      return next;
    });
  }, []);

  /** Jump the board to an item and flash it — used by the "on the board" links. */
  const focusItem = useCallback(
    (code: string) => {
      setTab("roadmap");
      setFocusCode(code);
      setOpenCodes((s) => new Set(s).add(code));
      after(FOCUS_MS, () => setFocusCode(null));
    },
    [after],
  );

  return {
    /** The project this board belongs to; null until the workspace loads, or
     *  for a repo that isn't in a project. The thread needs it too — its chats
     *  are scoped to the project, not the repo. */
    projectId,
    // board
    items,
    ghosts,
    counts,
    shipped,
    loading,
    /** No project row for this repo — the board can be read but not written,
     *  so the write affordances stay out of the way. */
    readOnly: workspaceReady && projectId == null,
    error,
    clearError,
    map: PRODUCT_MAP,
    tab,
    setTab,
    openCodes,
    toggleItem,
    focusCode,
    focusItem,
    landed,
    /** Why a queued item isn't moving, by item id. */
    notes,
    addItem,
    editItem,
    moveItem,
    removeItems,
    acceptItems,
    queueItems,
    unqueueItems,
    /** Definitions + the project default, for the queue affordance and the
     *  item form. */
    workflows,
  };
}

export type RoadmapState = ReturnType<typeof useRoadmap>;
