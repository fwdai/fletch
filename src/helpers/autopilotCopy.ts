// ── Autopilot, in words ───────────────────────────────────────────────────────
//
// Autopilot is not a mode the user sees; it is how an agent behaves on an open
// PR (per project, in Project Settings; pausable per workspace from the Git
// panel). So nothing here names it: every line is a fact about the PR, phrased
// as what happened. One home for the phrasing so the Git panel's status line,
// the sidebar tooltip and the history log can never disagree.

import { type GiveUpReason, RUNG_BUDGET } from "@/autopilot";
import type { DelegationKind } from "@/delegation";

/** The rung as a thing on the PR, not as an action name. Partial because a log
 *  row can name a rung autopilot doesn't drive, and the raw kind is an honest
 *  fallback for those. */
const RUNG_NOUN: Partial<Record<DelegationKind, string>> = {
  "fix-checks": "failing checks",
  resolve: "conflicts",
  "update-branch": "branch update",
  "resolve-comments": "review comments",
};

export const rungNoun = (kind: DelegationKind): string => RUNG_NOUN[kind] ?? kind;

/** Why autopilot gave up on a rung — the one history row a returning user is
 *  looking for. Past tense: this already happened, and autopilot is simply
 *  waiting for the situation to change. */
export function gaveUpLabel(reason: GiveUpReason, rung: DelegationKind | null): string {
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
    case "no-evidence":
      return "No CI result came back";
  }
}
