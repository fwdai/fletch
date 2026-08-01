// The Roadmap surface's data model.
//
// The board's rows are real, persisted `roadmap_items` — their shape lives in
// `@/api` (src/api/types/roadmap.ts, mirroring src-tauri/src/roadmap/types.rs)
// and is re-exported here so the folder keeps importing from one place. What is
// defined here is what the *screen* adds on top: the display row the board
// draws, and the product map (still mock).
//
// A PM proposal is not a separate kind of row: `roadmap_propose` persists it
// with `status: "proposed"`, so a ghost on the board is an ordinary item that
// hasn't been accepted yet.

import type { Horizon, ItemSize, ItemSource, ItemStatus, RoadmapItem } from "@/api";

export type { Horizon, ItemSize, ItemSource, ItemStatus, RoadmapItem } from "@/api";

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
  size?: ItemSize;
  /** Product-map domain this belongs to (`MapDomain.id`). */
  area?: string;
  /** Acceptance criteria, rendered as a checklist. */
  accept?: string[];
  /** Codes this item must land after. */
  deps?: string[];
  /** Optional grouping label when several items were shaped together. */
  epic?: string;
  /** Agent working it — only meaningful while `status === "active"`. */
  agent?: string;
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
    size: item.size ?? undefined,
    area: item.area ?? undefined,
    accept: item.accept.length ? item.accept : undefined,
    deps: item.deps.length ? item.deps : undefined,
    epic: item.epic ?? undefined,
    agent: item.agent_id ?? undefined,
    item,
  };
}

/** A slice of the codebase the PM agent knows about, shown on the Product map
 *  tab. `heat` is how much recent work has touched it. */
export interface MapDomain {
  id: string;
  label: string;
  note: string;
  files: number;
  /** Roadmap items currently pointing at this domain. */
  items: number;
  heat: "hot" | "warm" | "cool";
}

export const SIZE_HINT: Record<ItemSize, string> = {
  XS: "a few minutes",
  S: "under an hour",
  M: "half a day",
  L: "multi-day",
};

/** Board groups, in display order. The same labels drive the header stats. */
export const HORIZONS: { id: Horizon; label: string; note: string }[] = [
  { id: "now", label: "In flight", note: "being built" },
  { id: "next", label: "Next", note: "queued" },
  { id: "later", label: "Later", note: "backlog" },
];

export const HEAT_LABEL: Record<MapDomain["heat"], string> = {
  hot: "active",
  warm: "planned",
  cool: "quiet",
};
