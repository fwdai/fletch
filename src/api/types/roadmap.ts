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

export type ItemSize = "XS" | "S" | "M" | "L";

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
  size: ItemSize | null;
  /** Product-map domain this belongs to. */
  area: string | null;
  source: ItemSource;
  /** Grouping label when several items were shaped together. */
  epic: string | null;
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
  size?: ItemSize | null;
  area?: string | null;
  source?: ItemSource;
  epic?: string | null;
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

/** A partial update. An omitted key is left alone; an explicit `null` on a
 *  nullable column clears it — so `{ size: null }` unsets the size while `{}`
 *  changes nothing. `code` and `project_id` are not patchable. */
export interface RoadmapItemPatch {
  title?: string;
  why?: string;
  horizon?: Horizon;
  status?: ItemStatus;
  source?: ItemSource;
  accept?: string[];
  deps?: string[];
  size?: ItemSize | null;
  area?: string | null;
  epic?: string | null;
  agent_id?: string | null;
  workflow_def_id?: string | null;
  run_id?: string | null;
  pr_url?: string | null;
  pr_number?: number | null;
}
