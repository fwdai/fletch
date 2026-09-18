// PM proposals: the tickets a Project Manager suggests while a planning chat is
// running. Each one is a real `roadmap_items` row parked at `proposed` — a ghost
// that exists on the board but is not a backlog item until the user says so, so
// the phone's job is to put the decision where the idea was had.
//
// Kept per project rather than per chat, because that is how the host keys them
// and because two chats on one project propose onto the same board.

import type { RoadmapItem } from "@desktop/api/types/roadmap";
import type { Api } from "../api";
import type { MobileState } from "./index";

type Set = (partial: Partial<MobileState> | ((s: MobileState) => Partial<MobileState>)) => void;
type Get = () => MobileState;

export interface ProposalsSlice {
  /** Pending ghosts per project id, newest first. Only `proposed` rows live
   *  here — anything else has been ruled on and belongs to the board. */
  proposals: Record<string, RoadmapItem[]>;
  loadProposals(projectId: string): Promise<void>;
  /** Say yes: `proposed → open`, and with `queue` hand it to the drainer in the
   *  same gesture. */
  acceptProposal(item: RoadmapItem, queue: boolean): Promise<void>;
  /** Say no. The row was never a roadmap item, so it goes rather than being
   *  ruled off the board — unless it has since been accepted elsewhere, in
   *  which case the host refuses and the card comes down on the truth. */
  discardProposal(item: RoadmapItem): Promise<void>;
  /** Fold a live `roadmap:item` in. A no-op for a project nothing is watching,
   *  so the event handler can call it unconditionally. */
  applyRoadmapItem(item: RoadmapItem): void;
  removeRoadmapItem(id: string): void;
}

/** What the slice borrows from the store it composes into, so it can be a module
 *  of its own without importing the store back. */
export interface ProposalsDeps {
  api: Api;
  /** Record a failure where the error UI can see it, then rethrow. */
  guard<T>(fn: () => Promise<T>): Promise<T>;
}

const newestFirst = (a: RoadmapItem, b: RoadmapItem) => b.created_at - a.created_at;

export function createProposalsSlice(set: Set, get: Get, deps: ProposalsDeps): ProposalsSlice {
  const { api, guard } = deps;

  /** Take one item out of whichever project holds it, leaving every other
   *  project's array at the same reference. */
  const drop = (id: string) =>
    set((s) => {
      const projectId = Object.keys(s.proposals).find((pid) =>
        s.proposals[pid].some((i) => i.id === id),
      );
      if (!projectId) return {};
      return {
        proposals: {
          ...s.proposals,
          [projectId]: s.proposals[projectId].filter((i) => i.id !== id),
        },
      };
    });

  return {
    proposals: {},

    async loadProposals(projectId) {
      // Nothing to ask an older host, and asking would answer "unknown op".
      if (!get().hostSupports("roadmap_list_items")) return;
      try {
        const items = await api.roadmapListItems(projectId);
        set((s) => ({
          proposals: {
            ...s.proposals,
            // The board is read whole and narrowed here: only a ghost is a
            // question, and only questions belong in a conversation.
            [projectId]: items.filter((i) => i.status === "proposed").sort(newestFirst),
          },
        }));
      } catch {
        // Advisory, like the other list reads: the chat stays as it was and the
        // next mount or reconnect tries again.
      }
    },

    async acceptProposal(item, queue) {
      return guard(async () => {
        // Conditional like the desktop's accept: admitting is only meaningful
        // while the row is still a proposal. `applied: false` means someone
        // ruled on it first — the row that comes back is the truth either way,
        // and folding it in is what takes the card down.
        const update = await api.roadmapUpdateItem(item.id, { status: "open" }, "proposed", queue);
        get().applyRoadmapItem(update.item);
      });
    },

    async discardProposal(item) {
      return guard(async () => {
        // Conditional like the accept: the host deletes only while the row is
        // still a proposal. `applied: false` with a row means someone ruled on
        // it first — folding that row in takes the card down (it is no longer a
        // ghost) without anything having been deleted. `applied: false` with no
        // row is a ghost that is already gone.
        const { applied, item: current } = await api.roadmapDiscardProposal(item.id);
        if (!applied && current) get().applyRoadmapItem(current);
        else get().removeRoadmapItem(item.id);
      });
    },

    applyRoadmapItem(item) {
      // Still a ghost: upsert it, so a PM revising its own suggestion replaces
      // the card rather than stacking a second one. Any other status means the
      // item was accepted or moved on, here or on the Mac — either way it is no
      // longer a decision this chat is waiting on.
      if (item.status !== "proposed") {
        drop(item.id);
        return;
      }
      set((s) => {
        const list = s.proposals[item.project_id] ?? [];
        const next = list.some((i) => i.id === item.id)
          ? list.map((i) => (i.id === item.id ? item : i))
          : [item, ...list];
        return { proposals: { ...s.proposals, [item.project_id]: next } };
      });
    },

    removeRoadmapItem: drop,
  };
}
