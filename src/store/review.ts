// The local review loop, mirrored from the host. The host owns the lifecycle
// (`review::*`): it spawns the hidden reviewer over a checkout, runs its
// turns, turns its verdict into local review threads and says where the review
// stands. This slice holds that state per checkout — fed by `review:changed`
// and by `get_reviews` on bootstrap, reconnect and environment switch, like
// `delegations` and `autopilot` — plus the two settings that decide when and
// with whom a review runs.

import { api, type ReviewState } from "@/api";
import { REVIEWER_NAME } from "@/starterPack/presets";
import type { CustomAgent } from "@/storage/customAgents";
import { setSetting } from "@/storage/settings";
import { newestWins } from "@/util/newestWins";
import { activeEnvironment, forActiveEnvironment } from "./environments";
import { gateReason } from "./gates";
import { checkoutKey } from "./git";
import type { SliceCreator } from "./types";

/** The settings keys the host reads too, so the two sides agree by name. */
export const REVIEW_ON_PR_OPEN_KEY = "review.on_pr_open";
export const REVIEW_AGENT_KEY = "review.agent";

export { REVIEWER_NAME };

/** Which custom agent reviews: the picked one when it still exists, else the
 *  one named "Reviewer" — the host's own fallback, mirrored so the picker and
 *  the panel show the agent the host would actually run. Null when neither
 *  exists: nothing reviews until one is created. */
export function resolveReviewer(agents: CustomAgent[], pickedId: string): CustomAgent | null {
  if (pickedId) {
    const picked = agents.find((a) => a.id === pickedId);
    if (picked) return picked;
  }
  return agents.find((a) => a.name.trim().toLowerCase() === REVIEWER_NAME.toLowerCase()) ?? null;
}

export interface ReviewSlice {
  /** The host's state per checkout, keyed by `checkoutKey(agent_id, subdir)`.
   *  Absent = the host has nothing to say about it (never reviewed). */
  reviews: Record<string, ReviewState>;
  /** Run a review when a PR opens on a checkout (`review.on_pr_open`). On by
   *  default: only an explicit "false" turns it off. */
  reviewOnPrOpen: boolean;
  /** The custom agent id picked as the reviewer (`review.agent`); "" = unset,
   *  which the host resolves to the agent named "Reviewer". */
  reviewAgentId: string;

  /** Replace the mirror with the host's table (`get_reviews`). */
  loadReviews: () => Promise<void>;
  /** Fold one `review:changed` into the mirror: it replaces the checkout's row. */
  applyReviewChanged: (state: ReviewState) => void;
  /** Start (or re-run) a review on a checkout. A refusal lands in `lastError`. */
  startReview: (agentId: string, subdir?: string, customAgentId?: string | null) => Promise<void>;
  stopReview: (agentId: string, subdir?: string) => Promise<void>;
  /** Resolve a local thread by hand. The host answers with `pr:threads_changed`.
   *  Resolves false on a refusal, which lands in `lastError`. */
  dismissReviewThread: (agentId: string, threadId: string, subdir?: string) => Promise<boolean>;
  /** Open the hidden reviewer's transcript in the center pane. */
  openReviewer: (reviewerAgentId: string, projectId: string) => Promise<void>;
  setReviewOnPrOpen: (on: boolean) => Promise<void>;
  setReviewAgent: (customAgentId: string) => Promise<void>;
}

const keyOf = (r: ReviewState) => checkoutKey(r.agent_id, r.subdir ?? undefined);

/** Only the newest `get_reviews` read may land (see `newestWins`). */
const loads = newestWins();

export const createReviewSlice: SliceCreator<ReviewSlice> = (set, get) => ({
  reviews: {},
  reviewOnPrOpen: true,
  reviewAgentId: "",

  loadReviews: async () => {
    // A host from before the review loop answers no `get_reviews`: nothing to
    // mirror, and no error worth raising for it.
    if (gateReason(activeEnvironment(), "review")) {
      set({ reviews: {} });
      return;
    }
    const claim = loads.claim();
    let rows: ReviewState[] | undefined;
    try {
      rows = await forActiveEnvironment(() => api.getReviews());
    } catch {
      // Best effort, like the other resync reads: the events keep it current.
      return;
    }
    // `undefined`: the environment changed under the read, so it is not ours.
    if (!rows || !claim.current()) return;
    const reviews: Record<string, ReviewState> = {};
    for (const row of rows) reviews[keyOf(row)] = row;
    set({ reviews });
  },

  applyReviewChanged: (state) => set((s) => ({ reviews: { ...s.reviews, [keyOf(state)]: state } })),

  startReview: async (agentId, subdir, customAgentId) => {
    const gated = gateReason(activeEnvironment(), "review");
    if (gated) {
      get().setLastError(gated);
      return;
    }
    try {
      const state = await forActiveEnvironment(() =>
        api.reviewStart(agentId, subdir, customAgentId),
      );
      if (state) get().applyReviewChanged(state);
    } catch (e) {
      get().setLastError(`Couldn't start the review: ${e}`);
    }
  },

  stopReview: async (agentId, subdir) => {
    try {
      const state = await forActiveEnvironment(() => api.reviewStop(agentId, subdir));
      if (state) get().applyReviewChanged(state);
    } catch (e) {
      get().setLastError(`Couldn't stop the review: ${e}`);
    }
  },

  dismissReviewThread: async (agentId, threadId, subdir) => {
    try {
      await forActiveEnvironment(() =>
        api.reviewDismissThread(agentId, threadId, subdir, "dismissed from the Git panel"),
      );
      return true;
    } catch (e) {
      get().setLastError(`Couldn't dismiss the finding: ${e}`);
      return false;
    }
  },

  openReviewer: async (reviewerAgentId, projectId) => {
    // The reviewer is hidden from the sidebar (a purpose-tagged workspace, like
    // a Roadmap PM chat), so the snapshot has no record for it: fetch the
    // project's reviewer chats, register the one asked for off-sidebar — the
    // per-agent machinery resolves provider and repo by id — then select it.
    try {
      const chats = await forActiveEnvironment(() => api.listProjectChats(projectId, "reviewer"));
      const record = chats?.find((a) => a.id === reviewerAgentId);
      if (!record) {
        get().setLastError("The reviewer's transcript isn't available yet");
        return;
      }
      get().registerOffSidebarAgents([record]);
      get().selectAgent(record.id);
    } catch (e) {
      get().setLastError(`Couldn't open the reviewer: ${e}`);
    }
  },

  setReviewOnPrOpen: async (on) => {
    set({ reviewOnPrOpen: on });
    await setSetting(REVIEW_ON_PR_OPEN_KEY, on ? "true" : "false");
  },

  setReviewAgent: async (customAgentId) => {
    set({ reviewAgentId: customAgentId });
    await setSetting(REVIEW_AGENT_KEY, customAgentId);
  },
});
