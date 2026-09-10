// ── Autopilot, in words ───────────────────────────────────────────────────────
//
// Autopilot is not a mode the user sees; it is how an agent behaves on a PR
// (per project, in Project Settings). So nothing here names it: every line is a
// fact about the PR, phrased as what happened, because the next move is the
// user's. One home for the phrasing so the Git panel status line, the sidebar
// tooltip, the Mission Control card and the history log can never disagree.

import { RUNG_BUDGET, type StuckReason } from "@/autopilot";
import type { DelegationKind } from "@/delegation";

/** The rung as a thing on the PR, not as an action name. Partial because an
 *  escalation can name a rung autopilot doesn't drive (`needs-human` on a
 *  commit), and the raw kind is an honest fallback for those. */
const RUNG_NOUN: Partial<Record<DelegationKind, string>> = {
  "fix-checks": "failing checks",
  resolve: "conflicts",
  "update-branch": "branch update",
  "resolve-comments": "review comments",
};

export const rungNoun = (kind: DelegationKind): string => RUNG_NOUN[kind] ?? kind;

/** Why the agent stopped working on this PR by itself. */
export function stuckLabel(reason: StuckReason, rung: DelegationKind | null): string {
  const noun = rung ? rungNoun(rung) : null;
  switch (reason) {
    case "budget-spent": {
      const tries = rung ? RUNG_BUDGET[rung] : undefined;
      return noun && tries
        ? `Gave up on the ${noun} after ${tries} tries`
        : "Gave up after several tries";
    }
    case "no-progress":
      return noun ? `Last attempt on the ${noun} changed nothing` : "Last attempt changed nothing";
    case "needs-human":
      return "Needs a decision from you";
    case "disputed-review":
      return "Agent pushed back on a review comment";
    case "dirty-tree":
      return "Waiting on your uncommitted changes";
    case "no-evidence":
      return "No CI result came back";
  }
}
