// The Roadmap board's single source of truth: the project's items, and the
// mutations every surface on this screen goes through.
//
// The board is persisted, per project, in `roadmap_items` (src-tauri/src/roadmap).
// This hook loads it once for the current project and then keeps it live off the
// `roadmap:item` / `roadmap:item-deleted` events, upserting the full row by id.
// The load subscribes before it fetches and replays anything that arrived in
// between (see rowSync.ts), because this board has writers other than the user.
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
// hook never writes those; it only ever asks for `queued`, takes it back, or
// ships an `in_review` item by hand when the merge sweep can't see the merge.
//
// Holds (migration 0033) are the exception that proves the contract. They are the
// one thing the PM writes directly, because they only ever *stop* the queue — so
// a hold can arrive here on `roadmap:item` (an item's, carried on the row) or on
// `roadmap:project-hold` (the board's) without the user having ruled on anything.
// The reverse is impossible: releasing is a typed command with no RPC op behind
// it, so every release on this board is a click someone made.
//
// Order is the third thing the two parties share. `rank` (migration 0032) is what
// the board draws a group by *and* what the drainer dispatches by, so dragging a
// card up the list moves it up the queue. The user writes it directly (a drag);
// the PM can only propose a whole new sequence, which arrives here as one
// board-scoped ask the user accepts or declines — the same suggest-never-commit
// contract, one altitude up from a single card.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  type NewRoadmapItem,
  onRoadmapItem,
  onRoadmapItemDeleted,
  onRoadmapItemEvent,
  onRoadmapOrderProposal,
  onRoadmapOrderProposalDeleted,
  onRoadmapProjectHold,
  onRoadmapProjectHoldReleased,
  onRoadmapProposal,
  onRoadmapProposalDeleted,
  onRoadmapQueueNote,
  type RoadmapItem,
  type RoadmapItemEvent,
  type RoadmapItemPatch,
  type RoadmapItemReview,
  type RoadmapOrderProposal,
  type RoadmapProjectHold,
  type RoadmapProposal,
  type WfRun,
} from "@/api";
import { applyRowEvent, createRowSync, createSingleSync } from "@/rowSync";
import { useAppStore } from "@/store";
import { useRuns } from "@/workflows/run/useRuns";
import { insertEvent, mergeSnapshot } from "./itemHistory";
import { PRODUCT_MAP } from "./mockData";
import { buildNeedsYou, mergeLatest, upsertLatest } from "./NeedsYou/select";
import { reviewFeedbackPrompt } from "./reviewPrompt";
import type { Horizon } from "./types";
import { toBoardItem } from "./types";
import { useItemReviews } from "./useItemReviews";
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

/** Can this row's position in the order be changed — by a drag, or by an
 *  accepted PM reordering? Everything from `active` on has been dispatched, so
 *  its place in the queue is settled and moving it would mean nothing (the same
 *  three statuses the backend's `order::is_orderable` allows). */
const isOrderable = (i: RoadmapItem) =>
  i.status === "proposed" || i.status === "open" || i.status === "queued";

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
  // The review loop's two effects live in the workspace this page covers: a fix
  // agent is a draft in the sidebar, and sending one navigates there.
  const createDraft = useAppStore((s) => s.createDraft);
  const updateDraft = useAppStore((s) => s.updateDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  // The live run rows, from the same `wf:run` stream the sidebar follows — an
  // `active` card reads its run's status off this rather than polling anything
  // of its own. Cheap: one list fetch and one subscription, both event-driven.
  const allRuns = useRuns();

  const [rows, setRows] = useState<RoadmapItem[]>([]);
  /** The PM's pending asks against existing items (`roadmap:proposal` +
   *  `roadmap_list_proposals`) — at most one per item, replaced in place under
   *  a stable id. Loaded with the item snapshot through the same
   *  subscribe-then-fetch-then-replay sequencer, because the PM parks these
   *  mid-conversation exactly like it proposes ghost rows. */
  const [proposalRows, setProposalRows] = useState<RoadmapProposal[]>([]);
  /** The PM's pending ask to reorder the whole board, or null. One per project
   *  (`roadmap:order-proposal`, keyed by project rather than by row), replaced in
   *  place when the PM changes its mind. */
  const [orderProposal, setOrderProposal] = useState<RoadmapOrderProposal | null>(null);
  /** The board's own hold, or null (`roadmap:project-hold` +
   *  `roadmap_get_project_hold`). One per project like the order ask, and kept
   *  here rather than derived from the rows because it belongs to no row: nothing
   *  dispatches while it exists, and the banner above the board is what says so. */
  const [projectHold, setProjectHold] = useState<RoadmapProjectHold | null>(null);
  const [loading, setLoading] = useState(true);
  /** The last failure from a mutation with no form of its own to report into
   *  (a move, a delete, an accepted proposal). */
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<BoardTab>("roadmap");
  const [openCodes, setOpenCodes] = useState<ReadonlySet<string>>(() => new Set());
  const [focusCode, setFocusCode] = useState<string | null>(null);
  /** Codes highlighted because they just landed or just moved. */
  const [landed, setLanded] = useState<ReadonlySet<string>>(() => new Set());
  /** Why an item isn't moving on its own, by item id — the transient
   *  `roadmap:queue-note`. The drainer sends them for stuck *queued* rows; the
   *  merge sweep sends one for an *open* row whose PR closed without merging.
   *  Not persisted anywhere, on purpose: it's only true until the next tick,
   *  and the row's own next change supersedes it. */
  const [notes, setNotes] = useState<ReadonlyMap<string, string>>(() => new Map());
  /** Each item's durable history, newest first (`roadmap:item-event` +
   *  `roadmap_list_item_events`). Held lazily: an item's trail is fetched on
   *  first expand and followed live from then on — events for items nobody
   *  ever expanded are simply dropped, so the map only ever holds what some
   *  card has shown. */
  const [events, setEvents] = useState<ReadonlyMap<string, RoadmapItemEvent[]>>(() => new Map());
  /** The newest event of every item on the board, one row per item
   *  (`roadmap_latest_events` + every live `roadmap:item-event`). Held for all
   *  items, unlike `events`: the "Needs you" strip asks a board-wide question
   *  ("is this item's latest word `blocked`?") and must see cards nobody
   *  expanded. One row per item, so it stays board-sized rather than
   *  history-sized. */
  const [latestEvents, setLatestEvents] = useState<RoadmapItemEvent[]>([]);
  /** Items whose history has been requested — the "followed live" set. In a
   *  ref because the event listener must read the current answer without
   *  resubscribing per expand. */
  const requestedEvents = useRef<Set<string>>(new Set());
  /** Which board the trails in `events` belong to. Bumped by the load effect on
   *  every project switch, so a history fetch that resolves after the switch can
   *  tell that its answer is about someone else's item and drop it — the map is
   *  reset synchronously there, but an in-flight request isn't. */
  const eventsGeneration = useRef(0);

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
      setRows((prev) => applyRowEvent(prev, { kind: "upsert", row }));
      // A note explains why a row isn't moving on its own. The moment the row
      // moves, whatever it said is history — drop it rather than leave a stale
      // excuse under a running item. Queued rows keep theirs: the drainer's
      // blocked-note must survive the row events around it. (A sweep note on an
      // open row survives arrival because it's emitted after the row event.)
      if (row.status !== "queued") dropNote(row.id);
    },
    [dropNote],
  );

  // Load the board: subscribe first, buffer during the fetch, then replay — so a
  // row the PM proposes while the board is loading cannot be lost. Rows change
  // from more than this screen (the PM agent's own writes, and later the run
  // queue), so the board follows the event rather than only its own command
  // results; the ordering the sequencer buys us is spelled out in rowSync.ts.
  useEffect(() => {
    if (!projectId) {
      setRows([]);
      setProposalRows([]);
      setOrderProposal(null);
      setProjectHold(null);
      setLatestEvents([]);
      setLoading(!workspaceReady);
      return;
    }
    let alive = true;
    setLoading(true);
    // A new board means new trails: what the previous project's cards loaded
    // says nothing about this one's items. The generation bump is what stops an
    // already-in-flight `loadEvents` from writing the old board's history into
    // this map.
    requestedEvents.current = new Set();
    eventsGeneration.current += 1;
    setEvents(new Map());
    setLatestEvents([]);

    // Unmounting mid-load must not write state, so every commit goes through
    // the same `alive` gate the fetch does.
    const sync = createRowSync<RoadmapItem>((update) => {
      if (alive) setRows(update);
    });
    const off = onRoadmapItem((row) => {
      if (row.project_id !== projectId) return;
      sync.push({ kind: "upsert", row });
      // Rows go through the sequencer; notes don't need to — they're a
      // separate map, and a stale excuse should vanish even mid-load.
      if (row.status !== "queued") dropNote(row.id);
    });
    // The proposal stream rides its own instance of the same sequencer: the
    // same two loss windows exist for it, and a replaced ask arrives as an
    // upsert under a stable id (see rowSync.ts).
    const psync = createRowSync<RoadmapProposal>((update) => {
      if (alive) setProposalRows(update);
    });
    const offProposal = onRoadmapProposal((p) => {
      if (p.project_id !== projectId) return;
      psync.push({ kind: "upsert", row: p });
    });
    const offProposalDeleted = onRoadmapProposalDeleted((id) => {
      // Addressed by proposal id; one we don't hold is simply never rendered.
      psync.push({ kind: "delete", id });
    });
    // The order ask and the board's hold are one row per board, so the list
    // sequencer would be the wrong shape — but the same two loss windows exist,
    // so the same subscribe-then-fetch-then-replay discipline applies, collapsed
    // to "last write wins" (see `createSingleSync`): anything that arrives during
    // the fetch is remembered and replayed over the snapshot instead of being
    // clobbered by it.
    const osync = createSingleSync<RoadmapOrderProposal>((p) => {
      if (alive) setOrderProposal(p);
    });
    const offOrder = onRoadmapOrderProposal((p) => {
      if (p.project_id !== projectId) return;
      osync.push(p);
    });
    const offOrderDeleted = onRoadmapOrderProposalDeleted((id) => {
      if (id !== projectId) return;
      osync.push(null);
    });
    // The hold matters most of the three: it is the reason nothing is
    // dispatching, so a lost one leaves a board that looks broken rather than
    // held — and a lost *release* leaves a banner over a board that is running.
    const hsync = createSingleSync<RoadmapProjectHold>((h) => {
      if (alive) setProjectHold(h);
    });
    const offHold = onRoadmapProjectHold((h) => {
      if (h.project_id !== projectId) return;
      hsync.push(h);
    });
    const offHoldReleased = onRoadmapProjectHoldReleased((id) => {
      if (id !== projectId) return;
      hsync.push(null);
    });
    const offDeleted = onRoadmapItemDeleted((id) => {
      sync.push({ kind: "delete", id });
      dropNote(id);
      // The backend cascades the row's history away; drop ours too, and let a
      // reused id (there are none, but the map shouldn't bet on that) refetch.
      requestedEvents.current.delete(id);
      setEvents((prev) => {
        if (!prev.has(id)) return prev;
        const next = new Map(prev);
        next.delete(id);
        return next;
      });
      setLatestEvents((prev) =>
        prev.some((e) => e.item_id === id) ? prev.filter((e) => e.item_id !== id) : prev,
      );
    });
    // History rows, appended only to trails some card already loaded — an
    // event for a never-expanded item is dropped here and refetched whole on
    // that item's first expand, which keeps the map leak-free.
    const offEvent = onRoadmapItemEvent((e) => {
      if (e.project_id !== projectId) return;
      // Before the lazy-trail gate: the strip follows every item's *newest*
      // event, including items whose card nobody ever opened. Merged rather than
      // buffered — "newest per item" is order-independent, so an event that
      // arrives during the snapshot fetch survives it without a replay.
      setLatestEvents((prev) => upsertLatest(prev, e));
      if (!requestedEvents.current.has(e.item_id)) return;
      setEvents((prev) => new Map(prev).set(e.item_id, insertEvent(prev.get(e.item_id) ?? [], e)));
    });
    // Addressed by item id, and every id on this board belongs to this project,
    // so no extra filtering is needed — a note for a row we don't hold is
    // simply never rendered.
    const offNote = onRoadmapQueueNote(({ item_id, note }) => {
      setNotes((prev) => new Map(prev).set(item_id, note));
    });

    void (async () => {
      // Registration has to be awaited, not just started: an event emitted
      // before `listen` resolves never reaches us at all. Every stream whose
      // loss the user would see is in here — the history rows and the queue
      // notes included: a note is deduped against the row version it describes
      // (see the drainer's `say`), so one missed during the load is not resent
      // and the card is left with no explanation at all.
      await Promise.all([
        off,
        offDeleted,
        offProposal,
        offProposalDeleted,
        offOrder,
        offOrderDeleted,
        offHold,
        offHoldReleased,
        // Awaited too, now that the strip decides from this stream: a `blocked`
        // emitted before registration resolves would otherwise be lost, and the
        // snapshot below can't backfill an event that fired after it was read.
        // Same for the notes stream — the drainer dedupes per row version, so a
        // note lost during the load is never resent.
        offEvent,
        offNote,
      ]);
      if (!alive) return;
      try {
        const [items, pending, order, latest, hold] = await Promise.all([
          api.roadmapListItems(projectId),
          api.roadmapListProposals(projectId),
          api.roadmapGetOrderProposal(projectId),
          api.roadmapLatestEvents(projectId),
          api.roadmapGetProjectHold(projectId),
        ]);
        if (!alive) return;
        sync.settle(items);
        psync.settle(pending);
        // Under whatever arrived live, per item — see `mergeLatest`.
        setLatestEvents((prev) => mergeLatest(prev, latest));
        osync.settle(order);
        hsync.settle(hold);
        setLoading(false);
      } catch (e) {
        if (!alive) return;
        // No snapshot to replay over — settle anyway so later events still
        // apply instead of piling up in the buffer.
        sync.settle();
        psync.settle();
        osync.settle();
        hsync.settle();
        setError(String(e));
        setLoading(false);
      }
    })();

    return () => {
      alive = false;
      void off.then((f) => f());
      void offDeleted.then((f) => f());
      void offProposal.then((f) => f());
      void offProposalDeleted.then((f) => f());
      void offOrder.then((f) => f());
      void offOrderDeleted.then((f) => f());
      void offHold.then((f) => f());
      void offHoldReleased.then((f) => f());
      void offEvent.then((f) => f());
      void offNote.then((f) => f());
    };
  }, [projectId, workspaceReady, dropNote]);

  /** Fetch an item's history once, on first expand. The listener above is
   *  already appending live rows for it from the moment this is called, and
   *  `mergeSnapshot` folds the two together by id — the same
   *  subscribe-then-fetch-then-replay shape the board load uses. A failed fetch
   *  un-requests the item so the next expand retries. */
  const loadEvents = useCallback(async (itemId: string) => {
    if (requestedEvents.current.has(itemId)) return;
    requestedEvents.current.add(itemId);
    // The board this fetch is for. A project switch resets the map and the
    // requested set, so a resolution that arrives afterwards is answering a
    // question about a board nobody is looking at — writing it would put another
    // project's trail under one of this project's item ids.
    const generation = eventsGeneration.current;
    try {
      const snapshot = await api.roadmapListItemEvents(itemId);
      if (eventsGeneration.current !== generation) return;
      setEvents((prev) =>
        new Map(prev).set(itemId, mergeSnapshot(prev.get(itemId) ?? [], snapshot)),
      );
    } catch {
      // History is a footnote: not worth the board's error bar. The card
      // simply has no trail until a later expand succeeds.
      if (eventsGeneration.current === generation) requestedEvents.current.delete(itemId);
    }
  }, []);

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
  /** The rows the board renders — everything but the shipped ones. The strip
   *  joins against these for the same reason the proposal lookup does: a card
   *  about a row nothing draws is a decision the user can't reach. */
  const onBoard = useMemo(() => rows.filter(isOnBoard), [rows]);
  const items = useMemo(() => onBoard.filter((r) => !isProposed(r)).map(toBoardItem), [onBoard]);
  /** Shipped items aren't on the board; the header carries the count. */
  const shipped = useMemo(() => rows.filter((r) => !isOnBoard(r)).length, [rows]);

  /** The PM's outstanding proposals, drawn on the board as ghosts until the
   *  user accepts or discards them. Kept out of `items` so they don't move a
   *  single count before that. */
  const ghosts = useMemo(() => rows.filter(isProposed).map(toBoardItem), [rows]);

  /** The PM's pending asks against existing items, by the item they target —
   *  the shape the card lookup wants, and one-per-item by construction (the
   *  backend replaces an item's ask in place). Restricted to items the board
   *  actually renders: an ask whose item advanced to `done` has no card to
   *  rule it from, and counting or quoting it would make a single orphan both
   *  invisible and immortal. The row itself survives in the DB; it comes back
   *  into view if the item ever returns to the board. */
  const proposals = useMemo(() => {
    const visible = new Set(onBoard.map((r) => r.id));
    const by = new Map<string, RoadmapProposal>();
    for (const p of proposalRows) {
      if (visible.has(p.item_id)) by.set(p.item_id, p);
    }
    return by as ReadonlyMap<string, RoadmapProposal>;
  }, [proposalRows, onBoard]);

  /** Every code the board holds, ghosts included — the PM quotes a code the
   *  moment it proposes one, so a chat chip must resolve before the user has
   *  ruled on the row.
   *
   *  Keyed on the codes themselves rather than on `rows`: the chat re-renders
   *  every markdown block when this set's identity changes, and a retitle or a
   *  status flip changes no code. */
  const codeKey = useMemo(() => [...new Set(rows.map((r) => r.code))].sort().join(" "), [rows]);
  const codes = useMemo(
    () => new Set(codeKey ? codeKey.split(" ") : []) as ReadonlySet<string>,
    [codeKey],
  );

  /** This project's live runs. Scoped here so a busy install's other runs reach
   *  neither a card nor the strip. */
  const runs = useMemo(
    () => allRuns.filter((r) => r.project_id === projectId),
    [allRuns, projectId],
  );

  /** The runs behind the board's items, by run id — what a card needs to say
   *  more than "running": the run's name, and why it stopped. */
  const runsById = useMemo(
    () => new Map(runs.map((r) => [r.id, r])) as ReadonlyMap<string, WfRun>,
    [runs],
  );

  /** Live review state for the board's `in_review` items, by item id — the CI
   *  rollup, the merge gate, and the unresolved threads a card renders and acts
   *  on. Polled here rather than in `gitSync` because a roadmap item has no
   *  checkout to key that machinery by; see useItemReviews.ts. */
  const { reviews, refreshReview } = useItemReviews(rows);

  /** The open decisions this board is waiting on the user for — paused runs and
   *  wedged items, as ordered cards. Derived from state this hook already holds,
   *  so a pause the queue hits shows up on the next `wf:run` with no polling of
   *  its own. The rules live in NeedsYou/select.ts. */
  const needsYou = useMemo(
    () => buildNeedsYou({ items: onBoard, runs, latestEvents, projectHold }),
    [onBoard, runs, latestEvents, projectHold],
  );

  /** The rows whose position in the order can still change, in priority order —
   *  what a drag may reorder, and the current order a proposal's preview is
   *  compared against. Same three statuses the backend's `order::is_orderable`
   *  allows.
   *
   *  Sorted here rather than trusted from the fetch: the snapshot arrives in rank
   *  order, but a live `roadmap:item` replaces a row *in place*, so a rank the
   *  user just dragged would otherwise leave the buffer's order stale. */
  const orderable = useMemo(() => rows.filter(isOrderable).sort((a, b) => a.rank - b.rank), [rows]);

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

  /** Move a card to another horizon, at a given position in the priority order —
   *  the drag that crosses groups.
   *
   *  One patch carrying both, deliberately: a horizon move is a planning fact
   *  ("this is next, not later"), so it should record itself as an edit in the
   *  item's history, and the rank it lands at is part of the same gesture. A
   *  rank move *within* a group is not a planning fact and goes through
   *  [`setRank`], which writes no history. */
  const moveItem = useCallback(
    (id: string, to: Horizon, rank?: number) =>
      guarded(async () => {
        const { item } = await api.roadmapUpdateItem(id, { horizon: to, rank });
        upsert(item);
        markLanded([item.code]);
      }),
    [guarded, markLanded, upsert],
  );

  /** Move a card within its group — bookkeeping, so no history line. Takes a
   *  list because the collision fallback rewrites a whole group's ranks (see
   *  rank.ts); the ordinary drag is one write. */
  const setRanks = useCallback(
    (writes: { id: string; rank: number }[]) =>
      guarded(async () => {
        for (const w of writes) upsert(await api.roadmapSetRank(w.id, w.rank));
      }),
    [guarded, upsert],
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

  /** Give this repo a project, so the board it is already showing becomes
   *  writable. The same entry point the sidebar's "Open a folder" uses
   *  (`NewProjectPopover` → `addWorkspaceRepo`), minus the folder picker — the
   *  path is the one this screen is open on. Deliberately not the New Project
   *  modal: that creates a *different*, brand-new repo, which is not what a user
   *  looking at this repo's read-only roadmap is asking for. Failures land on the
   *  store's `lastError` banner, as they do from the sidebar. */
  const addWorkspaceRepo = useAppStore((s) => s.addWorkspaceRepo);
  const makeProject = useCallback(
    () => void addWorkspaceRepo(repoPath),
    [addWorkspaceRepo, repoPath],
  );

  /** Accept proposed rows: `proposed → open` is the moment a suggestion becomes
   *  a roadmap item. The row (and its code) already exist, so nothing is
   *  re-created and nothing is renumbered — the code the PM quoted in the chat
   *  is the code that stays on the board. The other half of the decision is
   *  [`removeItems`].
   *
   *  `queue` is the "Accept & queue" click: accept and hand it to the drainer in
   *  one gesture. Where an accept *lands* is decided backend-side, not here — the
   *  project's autoqueue dial can queue it with `queue` unset, and a hold leaves
   *  it `open` even with `queue` set (holds trump the dial). So this reads the
   *  status off the row that comes back rather than assuming one. */
  const acceptItems = useCallback(
    (ids: string[], queue = false) =>
      guarded(async () => {
        const codes: string[] = [];
        for (const id of ids) {
          // Conditional like every other status transition: accepting is only
          // meaningful on a row that is still a proposal.
          const { applied, item } = await api.roadmapUpdateItem(
            id,
            { status: "open" },
            "proposed",
            queue,
          );
          upsert(item);
          if (applied) codes.push(item.code);
        }
        markLanded(codes);
      }),
    [guarded, markLanded, upsert],
  );

  /** Apply pending PM proposals — the user's "yes" on each. The backend rules
   *  in one lock scope per proposal: a stale ask (its item went `active` since)
   *  is dropped there and surfaces here as the error the bar shows, while the
   *  `roadmap:proposal-deleted` emit clears it off the card either way. */
  const acceptProposals = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        // A stale ask refuses individually; it mustn't take the rest of an
        // "Accept all" down with it, so rule on every id and report the first
        // refusal after the fact.
        let refusal: unknown = null;
        for (const id of ids) {
          try {
            await api.roadmapAcceptProposal(id);
          } catch (e) {
            refusal ??= e;
          }
        }
        if (refusal != null) throw refusal;
      }),
    [guarded],
  );

  /** Decline pending PM proposals. The items are untouched; each refusal lands
   *  in its item's durable history for the PM's next session to see. */
  const rejectProposals = useCallback(
    (ids: string[]) =>
      guarded(async () => {
        for (const id of ids) await api.roadmapRejectProposal(id);
      }),
    [guarded],
  );

  /** Apply the PM's proposed order — one ruling for the whole sequence. A stale
   *  ask (the board's orderable set changed since) is dropped backend-side and
   *  surfaces here as the error the board's bar shows; the reordered rows arrive
   *  on `roadmap:item` either way. */
  const acceptOrder = useCallback(
    () =>
      guarded(async () => {
        if (projectId) await api.roadmapAcceptOrderProposal(projectId);
      }),
    [guarded, projectId],
  );

  /** Decline the proposed order. The board is untouched. */
  const rejectOrder = useCallback(
    () =>
      guarded(async () => {
        if (projectId) await api.roadmapRejectOrderProposal(projectId);
      }),
    [guarded, projectId],
  );

  /** Hand items to the drainer: `open → queued`. From here the Rust side owns
   *  the item — it picks the highest-ranked queued item whose deps have landed,
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

  /** Stop the queue from building one item until it's released. Not an unqueue:
   *  the row keeps its status and its place in the order, so releasing is a
   *  one-click undo. The row comes back carrying the hold trio, so the card's
   *  chip and the strip's card both follow from the ordinary item stream. */
  const holdItem = useCallback(
    (id: string, reason: string) =>
      guarded(async () => {
        upsert(await api.roadmapHoldItem(id, reason));
      }),
    [guarded, upsert],
  );

  /** Lift one item's hold — the user's alone (the PM has an op to hold and none
   *  to release). The backend records what was lifted, so the trail reads as a
   *  pair rather than as an unexplained resumption. */
  const releaseItem = useCallback(
    (id: string) =>
      guarded(async () => {
        upsert(await api.roadmapReleaseItem(id));
      }),
    [guarded, upsert],
  );

  /** Take a handed-off item back off its agent — the undo of "Send to an agent".
   *  Clears `agent_id` and lands a history note naming the agent; the row is then
   *  the queue's to dispatch again. The backend re-checks the gate (something to
   *  take back, and nothing dispatched since), so a refusal shows up on the
   *  board's error bar rather than being swallowed. */
  const reclaimItem = useCallback(
    (id: string) =>
      guarded(async () => {
        upsert(await api.roadmapReclaimItem(id));
      }),
    [guarded, upsert],
  );

  /** Stop the whole board. Runs already in flight still settle — reflecting
   *  reality is not autonomy — so this freezes what *starts*, not what finishes.
   *  The banner and the strip both read the row this returns. */
  const holdProject = useCallback(
    (reason: string) =>
      guarded(async () => {
        if (projectId) setProjectHold(await api.roadmapHoldProject(projectId, reason));
      }),
    [guarded, projectId],
  );

  /** Let the board run again. Optimistic on the state the banner reads: the
   *  backend also emits the release, and both say the same thing. */
  const releaseProject = useCallback(
    () =>
      guarded(async () => {
        if (!projectId) return;
        await api.roadmapReleaseProject(projectId);
        setProjectHold(null);
      }),
    [guarded, projectId],
  );

  /** Ship an in-review item by hand: `in_review → done`. The sweep normally
   *  does this when the PR merges, but it can't always know — a revoked GitHub
   *  token, a deleted PR, a repo that left the project all read as "still
   *  open" forever. This is the escape hatch; conditional so a verdict the
   *  sweep landed a moment earlier wins. */
  const markDone = useCallback(
    (id: string) =>
      guarded(async () => {
        const { item } = await api.roadmapUpdateItem(id, { status: "done" }, "in_review");
        upsert(item);
      }),
    [guarded, upsert],
  );

  // ── the review loop (in_review items) ──────────────────────────────
  /** Merge an in-review item's PR from its card, when the gate allows it.
   *
   *  Deliberately does NOT ship the item: `in_review → done` keeps its single
   *  writer, the merge sweep, because a merge call is a *request* and only
   *  GitHub's answer is evidence it landed. The backend nudges the sweep right
   *  after, so the row ships within a beat rather than sitting for a tick; the
   *  card's own read is refreshed too, so the Merge button stops offering to
   *  merge an already-merged PR in the meantime. */
  const mergeItemPr = useCallback(
    (id: string) =>
      guarded(async () => {
        await api.roadmapMergeItemPr(id);
        await refreshReview(id);
      }),
    [guarded, refreshReview],
  );

  /** Hand a PR's unresolved review threads to a fresh agent, based on the PR's
   *  own branch — the card's "Fix review feedback".
   *
   *  A plain draft, not a hand-off and not a delegation. No `agent_id` is
   *  stamped: this item's builder is the run that opened the PR (and
   *  `roadmap_hand_off_item` refuses in-review items for exactly that reason),
   *  and the fix agent belongs to the pull request. The item stays `in_review`
   *  and the sweep still rules on shipment; what changes is one durable `note` on
   *  the trail, so "why is there a second agent on this branch" has an answer
   *  after a reload.
   *
   *  The draft's base is the PR's head branch, so the agent forks from the PR
   *  and its pushes update it. Without a head ref (a degraded read) the draft
   *  keeps the project's default base and the user can still pick the branch on
   *  the new-agent screen — worse, but not wrong.
   *
   *  Navigation happens last: a failed note write leaves the user on the board
   *  with the error, holding a draft they can still send. */
  const sendReviewFeedback = useCallback(
    (item: RoadmapItem, review: RoadmapItemReview) =>
      guarded(async () => {
        const threads = review.comments?.unresolved ?? [];
        const prompt = reviewFeedbackPrompt(item, threads);
        // Same emptiness the card's button reads, so the two can't disagree.
        if (!prompt) return;
        const draftId = await createDraft(repoPath, prompt);
        // `createDraft` already surfaced its own failure; don't double-report.
        if (!draftId) return;
        if (review.head_ref) updateDraft(draftId, { base: review.head_ref });
        await api.roadmapNoteReviewFeedback(item.id, threads.length);
        closeProjectScreen();
      }),
    [guarded, createDraft, updateDraft, closeProjectScreen, repoPath],
  );

  // ── board interaction ──────────────────────────────────────────────
  /** Expand/collapse a card. `itemId` (when the caller holds the row) is what
   *  triggers the item's one-time history fetch — on every toggle rather than
   *  only on opens, because `loadEvents` is idempotent and "is it open" would
   *  otherwise be a second source of truth here. */
  const toggleItem = useCallback(
    (code: string, itemId?: string) => {
      setOpenCodes((s) => {
        const next = new Set(s);
        if (!next.delete(code)) next.add(code);
        return next;
      });
      if (itemId) void loadEvents(itemId);
    },
    [loadEvents],
  );

  /** Jump the board to an item: switch to the roadmap tab, expand the row,
   *  scroll it into view (the Board's `focusCode` effect) and ring it for a
   *  moment. Called from the PM chat's code chips, which sit beside this board,
   *  and from the cross-screen request below. */
  const focusItem = useCallback(
    (code: string) => {
      setTab("roadmap");
      setFocusCode(code);
      setOpenCodes((s) => new Set(s).add(code));
      // The card lands expanded, so its trail must load exactly as if the
      // user had expanded it by hand — otherwise the history line the caller
      // is often pointing at ("its run failed") is invisible until a manual
      // collapse and re-expand.
      const row = rows.find((r) => r.code === code);
      if (row) void loadEvents(row.id);
      after(FOCUS_MS, () => setFocusCode(null));
    },
    [after, rows, loadEvents],
  );

  // A jump asked for from outside this screen — a run's roadmap chip
  // (`ui.focusRoadmapItem`), which opened the project page and left the code
  // behind. Consumed only by the board that actually holds the code, and
  // cleared as it fires: the request is a one-shot, and a board that has just
  // mounted for a different project must not swallow it.
  const roadmapFocusCode = useAppStore((s) => s.roadmapFocusCode);
  const clearRoadmapFocus = useAppStore((s) => s.clearRoadmapFocus);
  useEffect(() => {
    if (!roadmapFocusCode) return;
    // Gate on the *rendered* set, not the raw buffer: a `done` row is in
    // `rows` but leaves the board, so matching it would consume the request,
    // scroll nowhere, and ring nothing. (Callers already avoid sending done
    // items here, but a race between the jump and a merge sweep can ship the
    // item mid-flight.)
    if (!rows.some((r) => r.code === roadmapFocusCode && isOnBoard(r))) return;
    focusItem(roadmapFocusCode);
    clearRoadmapFocus();
  }, [roadmapFocusCode, rows, focusItem, clearRoadmapFocus]);

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
     *  so the write affordances stay out of the way. [`makeProject`] is the way
     *  out of it. */
    readOnly: workspaceReady && projectId == null,
    /** Make this repo a project, which is what a read-only board is missing. */
    makeProject,
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
    /** Each item's durable history, by item id — only for items whose card has
     *  been expanded at least once. */
    events,
    /** Pending PM proposals against existing items, by item id. */
    proposals,
    /** The PM's pending whole-board reordering, or null. */
    orderProposal,
    /** The board's own hold, or null — why nothing is dispatching. An item's own
     *  hold rides its row (`hold_reason`), so only this one needs its own state. */
    projectHold,
    /** Every row whose position can still change, in board order — the drag's
     *  domain and the order preview's reference. */
    orderable,
    /** The project's live workflow runs, by run id — an item's `run_id` resolves
     *  here for the card's pearl label and pause reason. */
    runsById,
    /** The board's open decisions, most-decidable-first — the "Needs you" strip.
     *  Empty when nothing is waiting, and the strip then renders nothing. */
    needsYou,
    /** Live review state for `in_review` items, by item id — the merge gate the
     *  card renders and the threads its fix action hands over. Absent for an item
     *  whose read hasn't landed (or degraded), which the card draws as no gate
     *  rather than as a clean one. */
    reviews,
    /** Every code on this board, for the PM chat's code linkifier — exact
     *  matches only, because the prefix varies per project. */
    codes,
    addItem,
    editItem,
    moveItem,
    setRanks,
    removeItems,
    acceptItems,
    acceptProposals,
    rejectProposals,
    acceptOrder,
    rejectOrder,
    queueItems,
    unqueueItems,
    reclaimItem,
    markDone,
    mergeItemPr,
    sendReviewFeedback,
    holdItem,
    releaseItem,
    holdProject,
    releaseProject,
    /** Definitions + the project default, for the queue affordance and the
     *  item form. */
    workflows,
  };
}

export type RoadmapState = ReturnType<typeof useRoadmap>;
