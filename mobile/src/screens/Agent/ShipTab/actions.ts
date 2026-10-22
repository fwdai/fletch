// The phone's half of the Git panel's dispatch: what each footer key does from
// here, mirroring the desktop's `useGitActions.runAction`. The footer itself is
// the desktop's (`footerModel.ts`), and so is the playbook a delegated key hands
// the agent (`actionPlan.ts`); a key this table has no entry for is one
// the phone cannot run, so `shipView` leaves it out of the footer and the gate
// refuses it. One table for both, so no button is drawn that a tap can't run.

import type { PrState } from "@desktop/api/types/pr";
import {
  type PlanFacts,
  type Playbook,
  playbookFor,
} from "@desktop/components/RightPanel/GitPanel/actionPlan";
import { GATES } from "@desktop/store/gates";

export type ShipOp =
  /** Hand a playbook to the coding agent (`delegateGit`). */
  | { kind: "delegate"; playbook: Playbook }
  /** The host's `merge_pr`, behind the two-tap arm. */
  | { kind: "merge" }
  | { kind: "archive" }
  | { kind: "github"; url: string }
  /** The manual commit / push / PR sheet (`openSheet("pr")`). */
  | { kind: "manual" };

/** What a playbook's params are read from, and the PR `view-pr` opens. */
export interface ShipFacts extends PlanFacts {
  pr: PrState | null;
}

/** The phone-only row that opens the PR sheet: the user's own message in place
 *  of the agent's. */
export const MANUAL_KEY = "manual";

/** The keys whose first step is a commit or a push: the ones the Changes tab
 *  shows, and the ones the PR sheet is a manual alternative to. */
export const COMMIT_KEYS: ReadonlySet<string> = new Set([
  "agent-commit",
  "agent-commit-push",
  "agent-commit-pr",
  "agent-open-pr",
  "push",
  "open-pr",
  "commit-direct",
  "commit-pr-direct",
]);

/** What `key` does on the phone, or null when the phone has no way to run it
 *  (pull, rebase, stash, discard, abort, publish, connect-github, focus-pr,
 *  delete-branch, clear-config, and the `loading` placeholder). */
export function shipOp(key: string, f: ShipFacts): ShipOp | null {
  // Every key the plan covers goes to the agent, `push` included: the store's
  // direct push (`pushToPr`) also commits a dirty tree under the PR's title,
  // which a "Push N commits" row must not.
  const playbook = playbookFor(key, f);
  if (playbook) return { kind: "delegate", playbook };
  switch (key) {
    case "merge":
      return { kind: "merge" };
    case "archive":
      return { kind: "archive" };
    case "view-pr":
      return f.pr ? { kind: "github", url: f.pr.url } : null;
    // A commit or PR with the user's own words: on the phone that is the PR
    // sheet, the auto-fill PR included (the sheet asks for its title).
    case "open-pr":
    case "commit-direct":
    case "commit-pr-direct":
    case MANUAL_KEY:
      return { kind: "manual" };
    default:
      return null;
  }
}

/** Why the host the phone drives can't run `key` from here, or null — the
 *  gate's `hostRefusal`. `delegateGit` needs no check: on a host from before
 *  `delegate_git` the store sends the trigger as a plain message, as it always
 *  was. GitHub's connection is not the phone's to know; a host without it
 *  refuses the push or PR itself, and the store puts that in `lastError`. */
export function shipRefusal(key: string, f: ShipFacts, canMerge: boolean): string | null {
  const op = shipOp(key, f);
  if (!op) return "Not available from the phone";
  if (op.kind === "merge" && !canMerge) return GATES.mergePr.reason;
  return null;
}
