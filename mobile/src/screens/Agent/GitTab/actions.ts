// The one table of git actions the phone offers for a checkout, in the order
// the footer shows them: the first is the primary button, the rest the quiet
// alternatives under it. Mirrors the desktop's choices (GitPanel/hooks/
// useGitActions.ts, readiness.ts): the same playbooks with the same params, so
// a tap here sends the trigger the Mac would.

import type { GitState } from "@desktop/api/types/git";
import type { CheckRun, PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import { describeMergeGate } from "@desktop/mergeGate";

export interface GitAction {
  key: string;
  label: string;
  /** `delegate` hands `playbook` + `params` to the coding agent as an app-action
   *  trigger; `merge` is a host op (`merge_pr`) with no agent turn behind it. */
  kind: "delegate" | "merge";
  playbook?: string;
  params?: Record<string, string>;
}

export interface GitActionsInput {
  git: GitState | null | undefined;
  pr: PrState | null | undefined;
  checks: PrChecks | null | undefined;
  threads: PrComments | null | undefined;
  /** The host answers `merge_pr` (`hostSupports`). */
  canMerge: boolean;
  /** The branch a PR targets — `baseOf(agent)`. */
  base: string;
}

/** The commit / push / open-PR family: the actions local work (uncommitted
 *  files, unpushed commits) creates. The Changes tab's footer shows only these;
 *  the Git tab's footer leads with them when they apply. */
const COMMIT_FAMILY = new Set(["commit-push", "push", "commit-pr", "open-pr"]);
export const isCommitAction = (a: GitAction) => COMMIT_FAMILY.has(a.key);

/** One check's outcome, as the desktop's ChecksSection reads it: not done yet
 *  is pending, a completed run that is neither a success nor a skip (neutral,
 *  skipped, stale, none) failed. A skip counts as passed here — it is not
 *  something to fix and not something to wait for. */
export function checkOutcome(run: CheckRun): "passed" | "failed" | "pending" {
  if (run.status !== "completed") return "pending";
  switch (run.conclusion) {
    case "success":
    case "neutral":
    case "skipped":
    case "stale":
    case null:
      return "passed";
    default:
      return "failed"; // failure, timed_out, cancelled, action_required, …
  }
}

const delegate = (key: string, label: string, params?: Record<string, string>): GitAction => ({
  key,
  label,
  kind: "delegate",
  playbook: key,
  ...(params ? { params } : {}),
});

/** Everything the footer could offer, most pressing first:
 *
 *  1. local conflicts → `resolve-conflicts`, and nothing else until they are
 *  2. uncommitted files / unpushed commits → the commit-push / push /
 *     commit-pr / open-pr playbook, as the Changes footer always chose it
 *  3. an open PR behind or conflicting with its base → `update-branch`
 *  4. an open PR with a failing check → `fix-checks`
 *  5. an open PR with review threads awaiting us → `resolve-comments`
 *  6. an open PR GitHub would take, on a host that can merge → `merge`
 *
 *  So the merge leads only when nothing else needs doing, and never shows at
 *  all unless the gate is open. */
export function gitActionsFor({
  git,
  pr,
  checks,
  threads,
  canMerge,
  base,
}: GitActionsInput): GitAction[] {
  const out: GitAction[] = [];
  const files = git?.files ?? [];
  const unpushed = git?.unpushed ?? 0;
  const open = pr?.state === "open" ? pr : null;

  // A conflicted tree is the only thing on offer, as on the desktop: a commit
  // would capture the markers, and merging the PR while the checkout cannot
  // even be reconciled locally is not a state to publish from.
  if (files.some((f) => f.kind === "conflicted")) {
    return [delegate("resolve-conflicts", "Resolve conflicts with agent")];
  }

  // The commit-* playbooks start with a commit, so a clean tree (only unpushed
  // commits) gets the plain push / open-pr playbook; with a PR already open,
  // "open PR" degrades to push, since that is what updates it. Only the two
  // that open a PR need to know the base, as on the desktop.
  if (files.length > 0 || unpushed > 0) {
    out.push(
      open
        ? files.length
          ? delegate("commit-push", `Commit & push to #${open.number} with agent`)
          : delegate("push", `Push to #${open.number} with agent`)
        : files.length
          ? delegate("commit-pr", "Commit & open PR with agent", { base })
          : delegate("open-pr", "Open PR with agent", { base }),
    );
  }

  if (!open) return out;

  const gate = describeMergeGate(checks?.merge_state ?? null, {
    checksFailed: checks?.failed ?? 0,
    mergeable: open.mergeable,
  });
  if (gate.needsUpdate) {
    out.push(delegate("update-branch", "Update branch with agent", { base }));
  }
  if (gate.situation === "checks-failing") {
    // Required failures by name when GitHub reports them; otherwise whichever
    // runs came back red, so the agent still knows where to look.
    const failing = checks?.required_failing.length
      ? checks.required_failing
      : (checks?.runs ?? []).filter((r) => checkOutcome(r) === "failed").map((r) => r.name);
    out.push(
      delegate("fix-checks", "Fix failing checks with agent", { failing: failing.join(", ") }),
    );
  }
  // Only the threads awaiting us: one we replied to last is a deliberate
  // push-back waiting on a human, and sending the agent back to it would
  // re-argue the same point (readiness.ts makes the same split).
  const awaiting = (threads?.unresolved ?? []).filter((t) => !t.we_replied_last).length;
  if (awaiting > 0) {
    out.push(
      delegate(
        "resolve-comments",
        `Resolve ${awaiting} review comment${awaiting === 1 ? "" : "s"} with agent`,
        { count: String(awaiting) },
      ),
    );
  }
  if (gate.mergeAllowed && canMerge) {
    out.push({ key: "merge", label: `Merge PR #${open.number}`, kind: "merge" });
  }
  return out;
}
