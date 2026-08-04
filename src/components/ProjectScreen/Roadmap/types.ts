// The Roadmap surface's data model.
//
// The board's rows are real, persisted `roadmap_items` — their shape lives in
// `@/api` (src/api/types/roadmap.ts, mirroring src-tauri/src/roadmap/types.rs)
// and is re-exported here so the folder keeps importing from one place. What is
// defined here is what the *screen* adds on top: the display row the board draws,
// and its groups. (The second tab's product brief needs nothing here — it renders
// the persisted `RoadmapBrief` markdown straight from `@/api`.)
//
// A PM proposal is not a separate kind of row: `roadmap_propose` persists it
// with `status: "proposed"`, so a ghost on the board is an ordinary item that
// hasn't been accepted yet.

import type { Horizon, ItemSource, ItemStatus, RoadmapItem } from "@/api";

export type { Horizon, ItemSource, ItemStatus, RoadmapItem } from "@/api";

/** A row as the board draws it — a persisted item flattened by [`toBoardItem`],
 *  with the DTO's nulls collapsed to `undefined` so the card can test each
 *  field with one `if`. */
export interface BoardItem {
  /** Short human id ("FLT-142"). Unique per project, and the board's row key. */
  code: string;
  title: string;
  horizon: Horizon;
  /** Why it's on the board — the one line that justifies its place. */
  why: string;
  status: ItemStatus;
  source: ItemSource;
  /** Free-text product area this belongs to — a label the PM may set, matched
   *  against nothing (the brief's domains are prose, not an enum). */
  area?: string;
  /** Acceptance criteria, rendered as a checklist. */
  accept?: string[];
  /** Codes this item must land after. */
  deps?: string[];
  /** The stored row behind this card — what every mutation addresses. */
  item: RoadmapItem;
}

/** Adapt a persisted row for the board. The DTO's nulls become `undefined` so
 *  the display type stays uniform with a ghost's optional fields. */
export function toBoardItem(item: RoadmapItem): BoardItem {
  return {
    code: item.code,
    title: item.title,
    horizon: item.horizon,
    why: item.why,
    status: item.status,
    source: item.source,
    area: item.area ?? undefined,
    accept: item.accept.length ? item.accept : undefined,
    deps: item.deps.length ? item.deps : undefined,
    // No `agent` field: the row's `agent_id` is an id, and the card needs the
    // *name*, which only the workspace list knows — it resolves it there (and
    // renders nothing for an agent that has since been deleted).
    item,
  };
}

/** Board groups, in display order. The same labels drive the header stats. */
export const HORIZONS: { id: Horizon; label: string; note: string }[] = [
  { id: "now", label: "In flight", note: "being built" },
  { id: "next", label: "Next", note: "queued" },
  { id: "later", label: "Later", note: "backlog" },
];
