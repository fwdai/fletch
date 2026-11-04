// ThreadView/composer.ts — what the thread's composer says when no step agent
// is up to talk to. Pure, so the copy is asserted rather than eyeballed: a
// composer that looks enabled and drops the message is worse than a disabled one.

import type { WfRun } from "../../../../api";

/** The reason the composer is inert — always names what the user is waiting for. */
export function disabledHint(run: Pick<WfRun, "status">): string {
  switch (run.status) {
    case "done":
      return "This run has finished.";
    case "failed":
      return "This run stopped. Its reason is above.";
    case "canceled":
      return "This run was canceled.";
    default:
      return "The next step starts automatically — you can talk to its agent once it's up.";
  }
}
