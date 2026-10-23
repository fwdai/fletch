import { invoke } from "../invoke";
import type { ReviewState } from "../types/review";

/** The local review loop: a hidden reviewer agent the host runs over a
 *  checkout, on PR open or on demand. Its findings arrive as local review
 *  threads (`PrComment.source === "local"`); these ops only drive the turn. */
export const reviewApi = {
  getReview: (agentId: string, subdir?: string) =>
    invoke<ReviewState | null>("get_review", { agentId, subdir }),
  /** Every checkout the host has a review state for — a client connecting
   *  mid-flight catches up from this, then follows `review:changed`. */
  getReviews: () => invoke<ReviewState[]>("get_reviews"),
  /** Start (or re-run) a review. `customAgentId` overrides the configured
   *  reviewer for this run; absent, the host resolves its default. */
  reviewStart: (agentId: string, subdir?: string, customAgentId?: string | null) =>
    invoke<ReviewState>("review_start", { agentId, subdir, customAgentId: customAgentId ?? null }),
  reviewStop: (agentId: string, subdir?: string) =>
    invoke<ReviewState>("review_stop", { agentId, subdir }),
  /** Resolve a local thread by hand, recording why. */
  reviewDismissThread: (agentId: string, threadId: string, subdir?: string, reason?: string) =>
    invoke<void>("review_dismiss_thread", { agentId, subdir, threadId, reason: reason ?? null }),
};
