import { invoke } from "../invoke";
import type {
  ItemStatus,
  NewRoadmapItem,
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapItemPatch,
  RoadmapItemUpdate,
} from "../types/roadmap";

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
   *  explicit `null` clears a nullable column.
   *
   *  `expectStatus` makes it a *conditional* transition: the patch lands only
   *  while the row still says that status, and a miss comes back as
   *  `applied: false` with the row as it actually is (nothing written, nothing
   *  broadcast). That is what keeps a status change sent off a stale board from
   *  overwriting one the Rust drainer made in the meantime — see `unqueueItems`
   *  in useRoadmap.ts. Without it the patch is unconditional and `applied` is
   *  always true. */
  roadmapUpdateItem: (id: string, patch: RoadmapItemPatch, expectStatus?: ItemStatus) =>
    invoke<RoadmapItemUpdate>("roadmap_update_item", {
      id,
      patch,
      // Always sent, so the argument is present-and-null rather than absent.
      expectStatus: expectStatus ?? null,
    }),
  /** Remove an item from the board. */
  roadmapDeleteItem: (id: string) => invoke<void>("roadmap_delete_item", { id }),
  /** One item's durable history, newest first. Fetched lazily on first card
   *  expand; live rows arrive on `roadmap:item-event`. */
  roadmapListItemEvents: (itemId: string) =>
    invoke<RoadmapItemEvent[]>("roadmap_list_item_events", { itemId }),
};
