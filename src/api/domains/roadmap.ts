import { invoke } from "../invoke";
import type { NewRoadmapItem, RoadmapItem, RoadmapItemPatch } from "../types/roadmap";

/** Per-project roadmap storage (`roadmap_*`, src-tauri/src/roadmap). Every
 *  mutation also broadcasts the row on `roadmap:item` / `roadmap:item-deleted`,
 *  so callers that already subscribe don't need to refetch after writing —
 *  see `useRoadmap`. */
export const roadmapApi = {
  /** Every item on a project's roadmap, oldest first. Includes `done` items:
   *  the board hides them from the horizons and counts them as shipped. */
  roadmapListItems: (projectId: string) =>
    invoke<RoadmapItem[]>("roadmap_list_items", { projectId }),
  /** Add an item. The `code` is allocated backend-side (per-project, under the
   *  connection lock), which is why it isn't part of the payload. */
  roadmapCreateItem: (projectId: string, item: NewRoadmapItem) =>
    invoke<RoadmapItem>("roadmap_create_item", { projectId, item }),
  /** Patch an item and get the stored row back. Omitted keys are left alone; an
   *  explicit `null` clears a nullable column. */
  roadmapUpdateItem: (id: string, patch: RoadmapItemPatch) =>
    invoke<RoadmapItem>("roadmap_update_item", { id, patch }),
  /** Remove an item from the board. */
  roadmapDeleteItem: (id: string) => invoke<void>("roadmap_delete_item", { id }),
};
