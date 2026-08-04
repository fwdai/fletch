import { invoke } from "../invoke";
import type {
  ItemStatus,
  NewRoadmapItem,
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapItemPatch,
  RoadmapItemUpdate,
  RoadmapOrderProposal,
  RoadmapProposal,
} from "../types/roadmap";

/** Per-project roadmap storage (`roadmap_*`, src-tauri/src/roadmap). Every
 *  mutation also broadcasts the row on `roadmap:item` / `roadmap:item-deleted`,
 *  so callers that already subscribe don't need to refetch after writing —
 *  see `useRoadmap`. */
export const roadmapApi = {
  /** Every item on a project's roadmap in board order — by `rank`, which is also
   *  the order the queue dispatches in. Includes `done` items: the board hides
   *  them from the horizons and counts them as shipped. */
  roadmapListItems: (projectId: string) =>
    invoke<RoadmapItem[]>("roadmap_list_items", { projectId }),
  /** One item by id, or `null` when it's gone. For callers that hold an item id
   *  without its board — a workflow run's `roadmap_item_id` back-link. */
  roadmapGetItem: (itemId: string) => invoke<RoadmapItem | null>("roadmap_get_item", { itemId }),
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
  /** Move an item in the project's priority order — the board's drag within a
   *  horizon group. Bookkeeping, so it writes no history line (a *horizon* move
   *  is a planning fact and rides `roadmapUpdateItem` with the rank in the same
   *  patch). Returns the stored row. */
  roadmapSetRank: (itemId: string, rank: number) =>
    invoke<RoadmapItem>("roadmap_set_rank", { itemId, rank }),
  /** Record a manual hand-off: this item is being built by an agent the user
   *  spawned themselves ("Send to an agent"). Stamps `agent_id` and writes a
   *  `note` naming the agent; the status is untouched, so the queue still
   *  doesn't own the item. Returns the stored row. */
  roadmapHandOffItem: (itemId: string, agentId: string) =>
    invoke<RoadmapItem>("roadmap_hand_off_item", { itemId, agentId }),
  /** Take a handed-off item back off its agent — the undo of
   *  `roadmapHandOffItem`. Clears `agent_id` and writes a `note` naming the
   *  agent it came back from; the status is untouched, so the row is simply the
   *  queue's to dispatch again. Rejects when nothing was handed off, or when the
   *  item has since been dispatched (from `queued` on, the run is where that
   *  gets dealt with). Returns the stored row. */
  roadmapReclaimItem: (itemId: string) => invoke<RoadmapItem>("roadmap_reclaim_item", { itemId }),
  /** Remove an item from the board. */
  roadmapDeleteItem: (id: string) => invoke<void>("roadmap_delete_item", { id }),
  /** One item's durable history, newest first. Fetched lazily on first card
   *  expand; live rows arrive on `roadmap:item-event`. */
  roadmapListItemEvents: (itemId: string) =>
    invoke<RoadmapItemEvent[]>("roadmap_list_item_events", { itemId }),
  /** The newest event of every item on a project's board, newest first — one
   *  read for the board-wide question the "Needs you" strip asks (is this item's
   *  *latest* word `blocked`?), where the per-item fetch above would be one
   *  query per card and would miss every card nobody expanded. Fetched with the
   *  item snapshot; live rows arrive on `roadmap:item-event`. */
  roadmapLatestEvents: (projectId: string) =>
    invoke<RoadmapItemEvent[]>("roadmap_latest_events", { projectId }),
  /** The newest history row anywhere on a project's board, or `null` for a board
   *  that never moved. One row, not a trail: this answers "has the board changed
   *  since?" for the standup digest (see `standup.ts`), which compares it against
   *  the PM chat's last turn. */
  roadmapLatestEvent: (projectId: string) =>
    invoke<RoadmapItemEvent | null>("roadmap_latest_event", { projectId }),
  /** Every pending PM proposal on a project's board — fetched with the item
   *  snapshot; live rows arrive on `roadmap:proposal`. */
  roadmapListProposals: (projectId: string) =>
    invoke<RoadmapProposal[]>("roadmap_list_proposals", { projectId }),
  /** Apply a pending proposal — the user's "yes". Rejects with a message when
   *  the item raced past the gate (went `active` since the PM asked); the
   *  stale proposal is dropped backend-side either way and its removal arrives
   *  on `roadmap:proposal-deleted`. */
  roadmapAcceptProposal: (proposalId: string) =>
    invoke<void>("roadmap_accept_proposal", { proposalId }),
  /** Decline a pending proposal — the item is untouched, and the refusal lands
   *  in its durable history for the PM's next session to see. */
  roadmapRejectProposal: (proposalId: string) =>
    invoke<void>("roadmap_reject_proposal", { proposalId }),
  /** The project's pending whole-board order ask, or `null`. Fetched with the
   *  item snapshot; live rows arrive on `roadmap:order-proposal`. */
  roadmapGetOrderProposal: (projectId: string) =>
    invoke<RoadmapOrderProposal | null>("roadmap_get_order_proposal", { projectId }),
  /** Apply the PM's proposed order — ranks the whole sequence 1..n in one
   *  transaction. Rejects with a message when the board's orderable set changed
   *  since the ask (an item was claimed, a new one proposed); the stale ask is
   *  dropped backend-side either way and its removal arrives on
   *  `roadmap:order-proposal-deleted`. */
  roadmapAcceptOrderProposal: (projectId: string) =>
    invoke<void>("roadmap_accept_order_proposal", { projectId }),
  /** Decline the proposed order — the board is untouched. */
  roadmapRejectOrderProposal: (projectId: string) =>
    invoke<void>("roadmap_reject_order_proposal", { projectId }),
};
