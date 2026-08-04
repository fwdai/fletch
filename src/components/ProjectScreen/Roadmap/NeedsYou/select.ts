// NeedsYou/select.ts — the roadmap board's decision-card derivation. A pure
// function that composes state the board already holds (its rows, the `wf:run`
// list, and the newest durable event per item) into one ordered list of "needs
// you" cards. No store, no IPC, no React — so the join, the ordering and the
// reason vocabulary are unit-testable in isolation (select.test.ts) and the strip
// is a thin renderer over this.
//
// Modelled on MissionControl/queue.ts, deliberately: the two surfaces answer the
// same question at different altitudes (one fleet-wide, one for this project's
// board), so `workflow-approval` and `workflow-conflict` keep the names they have
// there — a card that reads one way in Mission Control must not read another way
// here. The roadmap-only reasons extend that vocabulary rather than renaming it.
//
// What is NOT here: dismissal. Mission Control lets the user hide a card until
// its signal changes because it is a fleet-wide inbox nobody finishes; this strip
// is one project's open decisions and every card has an action that resolves it,
// so a card leaves only when the underlying state moves.

import type { RoadmapItem, RoadmapItemEvent, WfPausedReason, WfRun } from "@/api";

/** Why a card is on the strip. `workflow-approval` / `workflow-conflict` are
 *  Mission Control's names for the same two signals (queue.ts `ReviewReason`);
 *  the rest are roadmap-scoped, named the same way — `workflow-*` for something
 *  a run is waiting on, `item-*` for something the board itself is wedged on. */
export type NeedsReason =
  | "workflow-question"
  | "workflow-approval"
  | "workflow-conflict"
  | "workflow-budget"
  | "item-blocked";

/** Ordering buckets, most-decidable-first — the same principle as queue.ts's
 *  BUCKET (a card you can clear in one gesture floats above one that needs a
 *  detour), and the same relative order for the two reasons both surfaces carry.
 *   0 question — answered inline on the card, one gesture, nothing else to open.
 *   1 approval — a dedicated evidence surface plus a one-click approve.
 *   2 conflict — a decision with a defined action, taken in the run.
 *   3 budget   — a decision with a defined action, but "is it worth more" needs
 *                the run's spend to answer.
 *   4 blocked  — the wedge that needs editing dependencies, not a ruling. */
export const BUCKET: Record<NeedsReason, number> = {
  "workflow-question": 0,
  "workflow-approval": 1,
  "workflow-conflict": 2,
  "workflow-budget": 3,
  "item-blocked": 4,
};

/** Which pauses are the *user's* to clear, and under which reason. A pause left
 *  out here is the engine's or the run's own business (`blocked_gate` retries,
 *  `stalled` is a supervisor state) and never opens a card. */
const PAUSE_REASON: Partial<Record<WfPausedReason, NeedsReason>> = {
  question: "workflow-question",
  approval: "workflow-approval",
  conflict: "workflow-conflict",
  budget_exceeded: "workflow-budget",
};

export interface NeedsCard {
  /** Stable id — `run:<runId>` for a run's pause, `blocked:<itemId>` for a
   *  board wedge. Drives the render key and nothing else: there is no dismissal
   *  to key on. */
  id: string;
  reason: NeedsReason;
  /** Ordering bucket (see BUCKET). Lower = more decidable = higher up. */
  bucket: number;
  /** Deterministic within-bucket tiebreak — most recent activity first. */
  activityAt: number;
  /** The item the card is about; every card names one (`CODE · title`). */
  itemId: string;
  code: string;
  title: string;
  /** The run whose pause this is, for the run-scoped reasons. */
  runId?: string;
  /** The pause as the engine named it, so the card can label it with the same
   *  `pausedLabel` the item card and the sidebar use. */
  pausedReason?: WfPausedReason;
  /** The event detail behind an `item-blocked` card — the named cycle
   *  ("MCA-101 → MCA-104 → MCA-101"). Absent when the writer recorded none. */
  detail?: string;
}

export interface NeedsInput {
  /** The rows the board renders (a shipped item has left the board, so its run's
   *  old pause is nobody's decision). */
  items: readonly RoadmapItem[];
  /** This project's live run rows. A run for an item not in `items` is skipped,
   *  which is also what scopes the strip: another project's runs never join. */
  runs: readonly WfRun[];
  /** The newest durable event per item (`roadmap_latest_events` + live
   *  `roadmap:item-event` rows). Several rows for one item are tolerated — the
   *  newest wins — so a caller need not pre-reduce. */
  latestEvents: readonly RoadmapItemEvent[];
}

/** The newest event per item id. `created_at` ties fall to the row seen last,
 *  matching the backend's `created_at DESC, rowid DESC` (events.rs
 *  `latest_per_item`) when the input arrives in that order. */
export function latestByItem(events: readonly RoadmapItemEvent[]): Map<string, RoadmapItemEvent> {
  const by = new Map<string, RoadmapItemEvent>();
  for (const e of events) {
    const held = by.get(e.item_id);
    if (!held || e.created_at >= held.created_at) by.set(e.item_id, e);
  }
  return by;
}

/** Fold one event into a latest-per-item list, keeping exactly one row per item.
 *  An older event for an item we already hold is dropped — the list is returned
 *  unchanged (same reference), so a stale row can't cost a re-render. The same
 *  event arriving twice is idempotent. */
export function upsertLatest(latest: RoadmapItemEvent[], e: RoadmapItemEvent): RoadmapItemEvent[] {
  const held = latest.find((x) => x.item_id === e.item_id);
  if (!held) return [...latest, e];
  if (held.id === e.id || e.created_at < held.created_at) return latest;
  return latest.map((x) => (x.item_id === e.item_id ? e : x));
}

/** Lay a fetched snapshot under whatever live events already arrived: per item
 *  the newer row wins, so the snapshot cannot clobber a `blocked` that landed
 *  while it was in flight — the window rowSync.ts closes for row streams, closed
 *  here by a merge instead of a buffer because "newest per item" is
 *  order-independent and needs no replay. */
export function mergeLatest(
  live: RoadmapItemEvent[],
  snapshot: readonly RoadmapItemEvent[],
): RoadmapItemEvent[] {
  let out = live;
  for (const e of snapshot) out = upsertLatest(out, e);
  return out;
}

/** Compose the strip from the board's current state. Pure and synchronous: it
 *  never waits on evidence (an approval card ranks high because it *has* a
 *  review surface, not because that surface has loaded). */
export function buildNeedsYou(input: NeedsInput): NeedsCard[] {
  const byId = new Map(input.items.map((i) => [i.id, i]));
  const cards: NeedsCard[] = [];

  // ── runs paused on something only the user can clear ──
  for (const run of input.runs) {
    if (run.status !== "paused" || !run.roadmap_item_id || !run.paused_reason) continue;
    const reason = PAUSE_REASON[run.paused_reason];
    if (!reason) continue;
    // Join miss: a run for an item this board doesn't render (another project's,
    // or one that shipped since). The run is still visible in Mission Control and
    // its own tab — it just isn't a decision *about this board*.
    const item = byId.get(run.roadmap_item_id);
    if (!item) continue;
    cards.push({
      id: `run:${run.id}`,
      reason,
      bucket: BUCKET[reason],
      activityAt: run.updated_at,
      itemId: item.id,
      code: item.code,
      title: item.title,
      runId: run.id,
      pausedReason: run.paused_reason,
    });
  }

  // ── items the board itself is wedged on ──
  // `blocked` is durable (A4 writes it when a queue head sits on a dependency
  // cycle), so this survives a reload the way the transient queue note doesn't.
  // Two gates, because either alone lies: an item whose *newest* word is
  // something else has moved on, and one that is no longer `queued` is not
  // waiting to dispatch (the user unqueued it, or it is already running).
  for (const e of latestByItem(input.latestEvents).values()) {
    if (e.kind !== "blocked") continue;
    // Also the join miss: an event for a row this board doesn't render.
    const item = byId.get(e.item_id);
    if (item?.status !== "queued") continue;
    cards.push({
      id: `blocked:${item.id}`,
      reason: "item-blocked",
      bucket: BUCKET["item-blocked"],
      activityAt: e.created_at,
      itemId: item.id,
      code: item.code,
      title: item.title,
      detail: e.detail ?? undefined,
    });
  }

  // Most-decidable-first, then most-recent-first, then id for a stable order.
  cards.sort(
    (a, b) =>
      a.bucket - b.bucket ||
      b.activityAt - a.activityAt ||
      (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
  return cards;
}
