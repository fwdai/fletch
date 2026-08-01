// The Roadmap board's single source of truth: the project's items, and the
// mutations every surface on this screen goes through.
//
// The board is persisted, per project, in `roadmap_items` (src-tauri/src/roadmap).
// This hook loads it once for the current project and then keeps it live off the
// `roadmap:item` / `roadmap:item-deleted` events, upserting the full row by id —
// the same fetch-once-then-upsert shape `useRuns` uses.
//
// The PM conversation is NOT here: it is a real agent chat, owned by the Thread
// column (see Thread/usePmChats.ts). What the two share is this contract — the
// PM proposes, the user commits, nothing reaches the board unaccepted — which is
// why `applyChanges` and the ghost rows live in this hook rather than in the
// thread that will (next slice) feed them.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type NewRoadmapItem,
  onRoadmapItem,
  onRoadmapItemDeleted,
  type RoadmapItem,
  type RoadmapItemPatch,
} from "@/api";
import { useAppStore } from "@/store";
import { PRODUCT_MAP } from "./mockData";
import type { BoardItem, Horizon, ProposalChange } from "./types";
import { toBoardItem } from "./types";

/** How long a row stays highlighted after landing on the board. */
const LANDED_MS = 2200;
/** How long a focused row keeps its ring after being jumped to. */
const FOCUS_MS = 2200;

export type BoardTab = "roadmap" | "map";

/** A shipped item leaves the board entirely and survives only as the header's
 *  count, so "on the board" is every status but `done`. */
const isOnBoard = (i: RoadmapItem) => i.status !== "done";

export function useRoadmap(repoPath: string) {
  // The board is per project, not per repo: a multi-repo project has one
  // roadmap. Resolved from the pinned-repo list the sidebar already loads.
  const projectId =
    useAppStore((s) => s.workspace?.projects.find((p) => p.path === repoPath)?.project_id) ?? null;
  // Until the workspace itself is loaded, a missing project_id means "not known
  // yet", not "no project" — telling a populated board it's empty for a frame
  // would flash the empty state at someone who has a roadmap.
  const workspaceReady = useAppStore((s) => s.workspace != null);

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
  /** Upsert a row by id, appending new ones — the backend lists oldest-first
   *  and a new row is the newest, so append keeps the two in the same order. */
  const upsert = useCallback((row: RoadmapItem) => {
    setRows((prev) =>
      prev.some((r) => r.id === row.id)
        ? prev.map((r) => (r.id === row.id ? row : r))
        : [...prev, row],
    );
  }, []);

  useEffect(() => {
    if (!projectId) {
      setRows([]);
      setLoading(!workspaceReady);
      return;
    }
    let alive = true;
    setLoading(true);
    api
      .roadmapListItems(projectId)
      .then((items) => {
        if (!alive) return;
        setRows(items);
        setLoading(false);
      })
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setLoading(false);
      });

    // Rows change from more than this screen (the PM agent's own writes, and
    // later the run queue), so the board follows the event rather than only
    // its own command results.
    const off = onRoadmapItem((row) => {
      if (row.project_id !== projectId) return;
      upsert(row);
    });
    const offDeleted = onRoadmapItemDeleted((id) => {
      setRows((prev) => prev.filter((r) => r.id !== id));
    });

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
    };
  }, [projectId, upsert, workspaceReady]);

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
  const items = useMemo(() => rows.filter(isOnBoard).map(toBoardItem), [rows]);
  /** Shipped items aren't on the board; the header carries the count. */
  const shipped = useMemo(() => rows.filter((r) => !isOnBoard(r)).length, [rows]);

  /** Rows a proposal would add, drawn on the board before anything is committed,
   *  and the moves it would make. Dormant until the PM's propose tool lands
   *  (next slice): the board already knows how to render both, so the wiring
   *  stays here rather than being rebuilt around it. */
  const ghosts: BoardItem[] = useMemo(() => [], []);
  const moves = useMemo<Extract<ProposalChange, { kind: "move" }>[]>(() => [], []);

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
      const row = await api.roadmapUpdateItem(id, patch);
      upsert(row);
      return row;
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
        const row = await api.roadmapUpdateItem(id, { horizon: to });
        upsert(row);
        markLanded([row.code]);
      }),
    [guarded, markLanded, upsert],
  );

  const removeItem = useCallback(
    (id: string) =>
      guarded(async () => {
        await api.roadmapDeleteItem(id);
        setRows((prev) => prev.filter((r) => r.id !== id));
      }),
    [guarded],
  );

  const clearError = useCallback(() => setError(null), []);

  // ── committing a proposal ──────────────────────────────────────────
  /** Turn a proposal into rows. An add becomes a real item — the proposed code
   *  was only ever a placeholder, so what the user sees afterwards is the code
   *  the backend allocated. A move addresses its row by code; one naming a code
   *  this project doesn't have is skipped rather than failing the whole commit.
   *  Returns the codes that actually landed.
   *
   *  Dormant with `ghosts`/`moves` above: the next slice's `roadmap_propose` RPC
   *  tool feeds real proposals through exactly this path. */
  const applyChanges = useCallback(
    async (changes: ProposalChange[]) => {
      if (!projectId) throw new Error("This repo isn't part of a project yet.");
      const codes: string[] = [];
      for (const c of changes) {
        if (c.kind === "add") {
          const row = await api.roadmapCreateItem(projectId, {
            title: c.item.title,
            why: c.item.why,
            horizon: c.item.horizon,
            size: c.item.size ?? null,
            area: c.item.area ?? null,
            source: "pm",
            epic: c.item.epic ?? null,
            accept: c.item.accept ?? [],
            deps: c.item.deps ?? [],
          });
          upsert(row);
          codes.push(row.code);
        } else {
          const target = rows.find((r) => r.code === c.code);
          if (!target) continue;
          const row = await api.roadmapUpdateItem(target.id, { horizon: c.to });
          upsert(row);
          codes.push(row.code);
        }
      }
      markLanded(codes);
      return codes;
    },
    [markLanded, projectId, rows, upsert],
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
    moves,
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
    addItem,
    editItem,
    moveItem,
    removeItem,
    applyChanges,
  };
}

export type RoadmapState = ReturnType<typeof useRoadmap>;
