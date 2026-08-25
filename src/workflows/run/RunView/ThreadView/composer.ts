// ThreadView/composer.ts — where the thread's one composer sends what the user
// types. Pure, so the routing is asserted rather than eyeballed: a composer that
// looks enabled and drops the message is worse than a disabled one.

import type { AgentRecord, WfRun } from "../../../../api";

export type ComposerRoute =
  /** A step agent is working — the normal chat composer talks to it. */
  | "live"
  /** The run is paused on an ask — the answer form resumes it. */
  | "question"
  /** Nothing can accept a message right now. */
  | "disabled";

type RunState = Pick<WfRun, "status" | "paused_reason">;

export function composerRoute(run: RunState, live: AgentRecord | undefined): ComposerRoute {
  // The question outranks a live agent: the run is waiting on the answer, and a
  // message to the step agent wouldn't resume it.
  if (run.status === "paused" && run.paused_reason === "question") return "question";
  return live ? "live" : "disabled";
}

/** The reason the composer is inert — always names what the user is waiting for. */
export function disabledHint(run: RunState): string {
  switch (run.status) {
    case "done":
      return "This run has finished.";
    case "failed":
      return "This run stopped. Its reason is above.";
    case "canceled":
      return "This run was canceled.";
    case "paused":
      return "Resolve the pause above to continue.";
    default:
      return "The next step starts automatically — you can talk to its agent once it's up.";
  }
}
