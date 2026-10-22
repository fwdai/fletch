// ── The action plan: which playbook a footer key hands the coding agent ──────
// The footer (`footerModel.ts`) names its actions by key; this module says what
// each delegated key asks the agent for and with which params, so the desktop's
// `runAction` and the phone's `shipOp` send the same trigger for the same tap.
// What runs it — the desktop's `delegateAction`, the phone's `delegateGit` — is
// each client's own; so is everything a key does without the agent.
//
// Pure, like `footerModel.ts`: no store or React.

import type { PrChecks, PrComments } from "@/api";
import type { DelegationKind } from "@/delegation";

/** What a playbook's params are read from. */
export interface PlanFacts {
  /** Mainline branch, for the playbooks that name it. */
  base: string;
  checks: PrChecks | null;
  comments: PrComments | null;
}

/** One trigger for the agent. `action` is the playbook name the trigger
 *  carries — the kind itself, except `resolve`'s `resolve-conflicts` (the same
 *  pair `readiness.ts`'s `Rung` holds). `params` carry only the dynamic context
 *  the playbook can't know. */
export interface Playbook {
  kind: DelegationKind;
  action: string;
  params?: Record<string, string>;
}

const playbook = (kind: DelegationKind, params?: Record<string, string>): Playbook =>
  params ? { kind, action: kind, params } : { kind, action: kind };

/** The playbook `key` hands the agent, or null for a key that isn't a
 *  delegation. `push` is the agent's push: the desktop runs it when there is no
 *  branch to push yet, the phone always. */
export function playbookFor(key: string, f: PlanFacts): Playbook | null {
  switch (key) {
    case "agent-commit":
      return playbook("commit");
    case "agent-commit-push":
      return playbook("commit-push");
    case "agent-commit-pr":
      return playbook("commit-pr", { base: f.base });
    case "agent-open-pr":
      return playbook("open-pr", { base: f.base });
    case "push":
      return playbook("push");
    case "agent-resolve":
      return { kind: "resolve", action: "resolve-conflicts" };
    case "agent-update-branch":
      // PR can't merge cleanly with the base (the base advanced). This is NOT
      // a local in-progress merge — the agent must sync the base in first.
      return playbook("update-branch", { base: f.base });
    case "agent-fix":
      return playbook("fix-checks", { failing: (f.checks?.required_failing ?? []).join(", ") });
    case "agent-resolve-comments": {
      // The threads waiting on us — not the ones we pushed back on, which a
      // person has to settle (the same split `readiness.ts` makes).
      const waiting = (f.comments?.unresolved ?? []).filter((t) => !t.we_replied_last).length;
      return playbook("resolve-comments", { count: String(waiting) });
    }
    default:
      return null;
  }
}
