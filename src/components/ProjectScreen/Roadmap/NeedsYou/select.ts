// NeedsYou/select.ts — the roadmap board's decision-card derivation. A pure
// function that composes state the board already holds (its rows, the `wf:run`
// list, the newest durable event per item, and the board's own hold) into one
// ordered list of "needs you" cards. No store, no IPC, no React — so the join,
// the ordering and the reason vocabulary are unit-testable in isolation
// (select.test.ts) and the strip is a thin renderer over this.
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
// so a card leaves only when the underlying state moves. A hold is the clearest
// case of that: Release *is* the resolution, and it is the user's alone — the PM
// can pull the brake and has no way to lift it, so this strip is the only place
// (with the card itself) a hold can end.

import type {
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapProjectHold,
  WfPausedReason,
  WfRun,
} from "@/api";

/** Why a card is on the strip. `workflow-approval` / `workflow-conflict` are
 *  Mission Control's names for the same two signals (queue.ts `ReviewReason`);
 *  the rest are roadmap-scoped, named the same way — `workflow-*` for something
 *  a run is waiting on, `item-*` for something the board itself is wedged on,
 *  and `project-*` for something about the whole board. */
export type NeedsReason =
  | "workflow-question"
  | "project-held"
  | "item-held"
  | "workflow-approval"
  | "workflow-conflict"
  | "workflow-budget"
  | "item-blocked";

/** Ordering buckets, most-decidable-first — the same principle as queue.ts's
 *  BUCKET (a card you can clear in one gesture floats above one that needs a
 *  detour), and the same relative order for the two reasons both surfaces carry.
 *   0 question     — answered inline on the card, one gesture, nothing else to open.
 *   1 project-held — one click, and it is the widest thing stopped: nothing on
 *                    this board dispatches until it is lifted.
 *   2 item-held    — one click, one item. Below the board-wide hold because it
 *                    stops less, above the gates because Release needs no
 *                    evidence surface: the reason is on the card.
 *   3 approval     — a dedicated evidence surface plus a one-click approve.
 *   4 conflict     — a decision with a defined action, taken in the run.
 *   5 budget       — a decision with a defined action, but "is it worth more"
 *                    needs the run's spend to answer.
 *   6 blocked      — the wedge that needs editing dependencies, not a ruling. */
export const BUCKET: Record<NeedsReason, number> = {
  "workflow-question": 0,
  "project-held": 1,
  "item-held": 2,
  "workflow-approval": 3,
  "workflow-conflict": 4,
  "workflow-budget": 5,
  "item-blocked": 6,
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
   *  board wedge, `held:<itemId>` / `project-held:<projectId>` for a hold. Drives
   *  the render key and nothing else: there is no dismissal to key on. */
  id: string;
  reason: NeedsReason;
  /** Ordering bucket (see BUCKET). Lower = more decidable = higher up. */
  bucket: number;
  /** Deterministic within-bucket tiebreak — most recent activity first. */
  activityAt: number;
  /** The item the card is about (`CODE · title`). Absent only for
   *  `project-held`, the one decision that belongs to the board rather than to
   *  any row — there is no card to jump to, so the strip renders a plain label
   *  instead of the item button. */
  itemId?: string;
  code?: string;
  title?: string;
  /** The run whose pause this is, for the run-scoped reasons. */
  runId?: string;
  /** The pause as the engine named it, so the card can label it with the same
   *  `pausedLabel` the item card and the sidebar use. */
  pausedReason?: WfPausedReason;
  /** Why, in the writer's own words: the named cycle behind `item-blocked`
   *  ("MCA-101 → MCA-104 → MCA-101"), or a hold's reason. Absent when the writer
   *  recorded none (a hold always has one — it is required). */
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
  /** The board's own hold, or null. Read from the row rather than from the event
   *  trail (there isn't one — a board-wide stop belongs to no item), which is
   *  also why it is the one card with no item. */
  projectHold?: RoadmapProjectHold | null;
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

  // ── the brake, at both scopes ──
  // Read off state, not off the trail: a hold is a *current* fact (the row's
  // trio, the hold row), so there is no "has the trail moved on" question to
  // ask — the card is here exactly while the hold is, and the one gesture that
  // clears it is the Release the card carries. Nothing else on the strip can be
  // resolved without leaving it.
  if (input.projectHold) {
    cards.push({
      id: `project-held:${input.projectHold.project_id}`,
      reason: "project-held",
      bucket: BUCKET["project-held"],
      activityAt: input.projectHold.created_at,
      detail: input.projectHold.reason,
    });
  }
  for (const item of input.items) {
    if (!item.hold_reason) continue;
    cards.push({
      id: `held:${item.id}`,
      reason: "item-held",
      bucket: BUCKET["item-held"],
      // The hold's own timestamp, not the row's `updated_at`: an edit to a held
      // item must not float its card back to the top of the bucket.
      activityAt: item.held_at ?? item.updated_at,
      itemId: item.id,
      code: item.code,
      title: item.title,
      detail: item.hold_reason,
    });
  }

  // ── items the board itself is wedged on ──
  // `blocked` is the durable kind for a *standing* blockage — one that will not
  // resolve without a person: a dependency cycle, a queued item with no workflow
  // to run it under, an invalid stored spec, a project with no repo, a watched
  // pull request that stopped answering. So this survives a reload the way the
  // transient queue note doesn't, and every one of those reaches the strip.
  //
  // Two gates, because either alone lies: an item whose *newest* word is
  // something else has moved on, and one that is no longer `queued` is not
  // waiting to dispatch (the user unqueued it, or it is already running). The
  // second gate is also why the unreachable-PR wedge lands on the card's trail
  // and not here — that item is `in_review`, and the decision it needs (merge it
  // by hand, or put it back on the board) is the review card's, not this strip's.
  //
  // Only `blocked` cards. The other endings a trail can carry — `run_failed`,
  // `run_canceled`, `run_deleted`, `pr_closed` — are records of something that
  // already finished, and each one leaves the item somewhere the user can act on
  // it (`open`, or back on the board). A card for them would be a notification,
  // not a decision.
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
