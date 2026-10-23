// The local review loop, as the host reports it: one `ReviewState` per
// checkout, replaced whole by every `review:changed` event (the same
// mirror-shape as `AutopilotCheckout`). The reviewer is a hidden agent of the
// chosen custom agent, adopting the coder's checkout; its findings arrive as
// `PrComment`s with `source: "local"` through the ordinary thread events, so
// this state is only about the review turn itself.

/** Where the review stands. `idle`: never reviewed, or nothing reviewed
 *  since the checkout was reset. `queued`: a start is held until the coder's
 *  turn (or a live delegation on the checkout) ends — the PR-open trigger
 *  usually lands mid-turn — and `review_stop` cancels it. `reviewing`: a
 *  reviewer turn is in flight. `findings`: the last review raised threads,
 *  some still open. `clean`: the last review approved (or every finding is
 *  resolved) and the code is as it was then. `stale`: the code changed since
 *  the last review — or during it, which `error` then says. `failed`: the
 *  turn ended without a verdict, or could not start — `error` says why. */
export type ReviewPhase =
  | "idle"
  | "queued"
  | "reviewing"
  | "findings"
  | "clean"
  | "stale"
  | "failed";

export type ReviewTrigger = "manual" | "pr-open" | "re-review";

export interface ReviewReviewer {
  custom_agent_id: string;
  /** The custom agent's name, also the author of its threads. */
  name: string;
  /** Its base provider id (`claude`, `codex`, …). */
  provider: string;
  model: string | null;
  /** The hidden reviewer workspace, once spawned; its transcript is reachable
   *  by this id. */
  agent_id: string | null;
}

export interface ReviewState {
  agent_id: string;
  /** The secondary repo; null for the agent's primary. */
  subdir: string | null;
  phase: ReviewPhase;
  trigger: ReviewTrigger | null;
  reviewer: ReviewReviewer | null;
  /** Reviews run so far on this checkout. */
  round: number;
  /** Local threads still open / resolved, over every round. */
  open: number;
  resolved: number;
  /** Epoch ms the current phase began — the timer's anchor while reviewing,
   *  the age of a clean verdict. */
  since_ms: number;
  reviewed_signature: string | null;
  error: string | null;
}
