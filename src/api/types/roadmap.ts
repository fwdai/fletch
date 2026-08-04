// Roadmap DTOs — the TypeScript mirror of the Rust `roadmap::types`
// (see src-tauri/src/roadmap/types.rs). These match the serde JSON exactly, so
// a row returned by `roadmap_list_items` and one delivered on the `roadmap:item`
// event are the same shape.
//
// The `*_json` TEXT columns are marshalled backend-side: `accept` and `deps`
// arrive as real arrays (empty, never null). Nullable columns arrive as `null`,
// never `undefined`.

/** Where an item sits on the board. `now` is being built, `next` is queued up,
 *  `later` is the backlog. Shipped items leave the board entirely. */
export type Horizon = "now" | "next" | "later";

/** Where the item came from — drawn as a glyph on the row. */
export type ItemSource = "user" | "pm" | "linear" | "github";

/** Item lifecycle. `proposed` is a PM suggestion the user hasn't accepted yet
 *  (a ghost row); `active` means an agent is on it right now; `done` items leave
 *  the board and become the header's "shipped" count. */
export type ItemStatus = "proposed" | "open" | "queued" | "active" | "in_review" | "done";

/** One `roadmap_items` row. */
export interface RoadmapItem {
  id: string;
  project_id: string;
  /** Short human id ("FLT-142"), unique per project and never reallocated. */
  code: string;
  /** Reserved for sub-items; always null today (no UI writes it). */
  parent_id: string | null;
  title: string;
  /** The one line that justifies the item's place on the board. */
  why: string;
  horizon: Horizon;
  status: ItemStatus;
  /** Where the item sits in the project's priority order — what the board sorts
   *  a horizon group by, and what the queue dispatches by. Fractional indexing:
   *  a card dropped between two neighbours takes the midpoint of their ranks, so
   *  one row is written per drag (see migration 0032). */
  rank: number;
  /** Product-map domain this belongs to. */
  area: string | null;
  source: ItemSource;
  /** Acceptance criteria, rendered as a checklist. */
  accept: string[];
  /** Codes this item must land after. */
  deps: string[];
  /** Workspace working it. */
  agent_id: string | null;
  workflow_def_id: string | null;
  run_id: string | null;
  pr_url: string | null;
  pr_number: number | null;
  created_at: number;
  updated_at: number;
}

/** The payload `roadmap_create_item` accepts. Only `title` is required; the
 *  backend defaults the rest (`later` / `open` / `user`) and allocates the code,
 *  which is why there is no `code` field here. */
export interface NewRoadmapItem {
  title: string;
  why?: string;
  horizon?: Horizon;
  status?: ItemStatus;
  area?: string | null;
  source?: ItemSource;
  accept?: string[];
  deps?: string[];
  /** Workflow this item is dispatched under when queued. Omitted (or null)
   *  means "the project's default at dispatch time". */
  workflow_def_id?: string | null;
}

/** Why a queued item isn't moving, as the drainer sees it
 *  (`roadmap:queue-note`, src-tauri/src/roadmap/drainer.rs).
 *
 *  Transient: nothing persists it, and the next change to the row makes it
 *  stale — which is exactly when the board drops it. Carries the code so a
 *  listener can name the item without holding the row. */
export interface RoadmapQueueNote {
  item_id: string;
  code: string;
  note: string;
}

/** Who moved an item — the four writers of `roadmap_items`, by surface: the
 *  typed commands (`user`), the propose RPC (`pm`), the queue drainer
 *  (`drainer`), and the merge sweep (`sweep`). */
export type RoadmapEventActor = "user" | "pm" | "drainer" | "sweep";

/** What happened to an item — one kind per transition, so a history line never
 *  re-derives meaning from a status pair. No member without a backend writer:
 *  `held`/`released` arrive with the holds slice (B5), and `discarded` is gone
 *  (discarding deletes the row, history and all; declining a PM ask writes a
 *  `note`).
 *
 *  `created` is the user-typed row's opening line, the mirror of the PM's
 *  `proposed`: without it a hand-built board has no history at all, and every
 *  "what changed since?" reader calls it unchanged. */
export type RoadmapEventKind =
  | "created"
  | "proposed"
  | "accepted"
  | "edited"
  | "queued"
  | "unqueued"
  | "dispatched"
  | "pr_opened"
  | "run_failed"
  | "shipped"
  | "abandoned"
  | "blocked"
  | "note";

/** One durable history row (`roadmap_item_events`, mirroring
 *  src-tauri/src/roadmap/events.rs). Every status transition writes exactly
 *  one; unlike the queue note, these survive a reload — a failed run's reason
 *  is still on the card tomorrow. */
export interface RoadmapItemEvent {
  id: string;
  item_id: string;
  /** Denormalized off the item so a board-scoped listener filters without
   *  holding the row. */
  project_id: string;
  actor: RoadmapEventActor;
  kind: RoadmapEventKind;
  /** Human-readable payload: a failure reason, a PR url, a workflow id. */
  detail: string | null;
  created_at: number;
}

/** What a pending PM proposal asks for: patch the item, or remove it. */
export type RoadmapProposalKind = "update" | "discard";

/** The delta an `update` proposal carries — the item's shape only, never its
 *  lifecycle (no `status`, no `code`, no run back-links; the backend refuses
 *  them). An omitted key is left alone; an explicit `null` on `area` clears
 *  it — the same wire semantics as `RoadmapItemPatch`. */
export interface RoadmapProposalPatch {
  title?: string;
  why?: string;
  horizon?: Horizon;
  area?: string | null;
  accept?: string[];
  deps?: string[];
}

/** One pending PM proposal (`roadmap:proposal` and `roadmap_list_proposals`
 *  carry the same shape, mirroring src-tauri/src/roadmap/proposals.rs). At
 *  most one exists per item; a newer ask replaces it under the *same id*, so
 *  upserting by id can never hold two for one item. Only the user's ruling
 *  (accept/reject) or the item's deletion removes it. */
export interface RoadmapProposal {
  id: string;
  item_id: string;
  /** Denormalized off the item so a board-scoped listener filters without
   *  holding the row. */
  project_id: string;
  kind: RoadmapProposalKind;
  /** The validated patch for an `update`; null for a `discard`. Its keys are
   *  exactly the fields the proposal changes. */
  patch: RoadmapProposalPatch | null;
  /** The PM's one-line rationale. Always present for a discard (the ask must
   *  explain itself); optional for an update. */
  note: string | null;
  created_at: number;
}

/** The PM's pending ask to reorder a whole board (`roadmap:order-proposal` and
 *  `roadmap_get_order_proposal`, mirroring src-tauri/src/roadmap/order.rs).
 *
 *  Board scoped, not item scoped: at most one per project, and a newer ask
 *  replaces it. `codes` is the *complete* order of the board's orderable items
 *  (`proposed | open | queued`) — the backend refuses a partial list, so the ask
 *  is unambiguous. The user's ruling re-validates the set and refuses if the
 *  board moved since. */
export interface RoadmapOrderProposal {
  project_id: string;
  /** Every orderable item's code, in the order the PM is asking for. */
  codes: string[];
  /** The PM's one-line rationale, shown on the board's order bar. */
  note: string | null;
  created_at: number;
}

/** The result of a patch: the stored row, and whether this patch is what stored
 *  it.
 *
 *  `applied` is false only for a *conditional* update (`expectStatus`) whose
 *  precondition missed — the row had already moved on, so nothing was written and
 *  nothing was broadcast. `item` is then the row as it actually is, which is what
 *  the caller should put on the board: its own snapshot was the stale one. */
export interface RoadmapItemUpdate {
  applied: boolean;
  item: RoadmapItem;
}

/** A partial update. An omitted key is left alone; an explicit `null` on a
 *  nullable column clears it — so `{ area: null }` unsets the area while `{}`
 *  changes nothing. `code` and `project_id` are not patchable.
 *
 *  Neither is `agent_id`: the hand-off and its undo are typed commands
 *  (`roadmapHandOffItem` / `roadmapReclaimItem`) because each writes a history
 *  note naming the agent, where a patch would record a bare "Edited". The
 *  backend ignores the key even if something sends it. */
export interface RoadmapItemPatch {
  title?: string;
  why?: string;
  horizon?: Horizon;
  status?: ItemStatus;
  source?: ItemSource;
  /** A new position in the priority order. Sent together with `horizon` when a
   *  drag crosses groups — one write, one planning fact, one history line. A
   *  rank move *within* a group goes through `roadmap_set_rank` instead, which
   *  writes no history. */
  rank?: number;
  accept?: string[];
  deps?: string[];
  area?: string | null;
  workflow_def_id?: string | null;
  run_id?: string | null;
  pr_url?: string | null;
  pr_number?: number | null;
}
