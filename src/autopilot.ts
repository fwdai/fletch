// ── Autopilot: drive an enrolled checkout toward landable, unattended ────────
//
// `readiness.ts` says what's wrong and what would fix it. This module decides
// whether to actually do it — and, crucially, when to stop.
//
// The unit here is a CYCLE, not a turn:
//
//   dispatch → agent turn → await evidence → verdict
//
// That distinction is the whole reason this module exists. A delegation's unit is
// one agent turn, and `delegationResolved("fix-checks")` deliberately returns
// false forever (CI takes minutes), so the delegation layer clears it on
// agent-idle with "checks are re-running". That is an honest thing to tell a
// human who clicked a button, but it is NOT a verdict — nobody has yet found out
// whether the fix worked. Autopilot waits for evidence, then judges.
//
// ── Why an unattended loop needs more than a ladder ─────────────────────────
// Every cycle costs an agent turn and a CI run. Two failure modes would burn them
// indefinitely:
//
//   1. A genuinely broken check. The agent "fixes" it, CI fails identically, the
//      ladder says fix-checks again, forever. Caught by the state SIGNATURE: a
//      cycle that ends on the signature it started from changed nothing.
//   2. A rung that oscillates (fix → push → a different failure → fix → the
//      first failure again). Caught by the per-rung attempt BUDGET.
//
// Both end in `escalate`, never in silence: the checkout goes back to the human
// with a reason, which is the only honest outcome for "I couldn't do this".
//
// Portable to Rust on the same terms as `readiness.ts` — pure, no framework, no
// clock of its own (`now` is a parameter). Enforced by `autopilot.test.ts`.

import type { GitState, VerificationReport } from "@/api";
import type { DelegationKind } from "@/delegation";
import { type LadderContext, nextRung, type ReadinessInput } from "@/readiness";

/** Rungs autopilot may run on its own, this slice.
 *
 *  Everything else the ladder returns escalates. Growing this set is how the
 *  later slices land — conflicts (`resolve`, `update-branch`), then review
 *  comments — each arriving with its own budget and evidence rule.
 *
 *  The commit / push / open-pr rungs are deliberately absent and not planned:
 *  auto-committing someone's working tree is a different risk class from fixing
 *  CI on work they already pushed. The ladder still computes them for the Git
 *  panel's button; autopilot declines them, which is why an enrolled checkout
 *  with uncommitted edits correctly does nothing. That also keeps `fix-checks`
 *  off a dirty tree — its playbook runs `git add -A`, which would otherwise
 *  sweep the user's in-flight edits into the agent's fix commit. */
export const AUTOPILOT_RUNGS: readonly DelegationKind[] = [
  "fix-checks",
  "resolve",
  "update-branch",
  "resolve-comments",
];

/** Cycles one rung gets on a checkout before autopilot gives up.
 *
 *  `fix-checks` gets three — a fix often needs a second look, and three is short
 *  of "not converging". The reconcile rungs get two: a merge either goes in or it
 *  needs a judgement call about intent that the second failure has already shown
 *  the agent isn't making. */
export const RUNG_BUDGET: Partial<Record<DelegationKind, number>> = {
  "fix-checks": 3,
  resolve: 2,
  "update-branch": 2,
  // Two. Each cycle posts real replies to a real conversation, so a loop that
  // isn't converging is not just wasted tokens — it's noise in someone's inbox.
  "resolve-comments": 2,
};

/** How a rung's cycle is judged.
 *
 *  `verify` — run the project's own tests/lints and believe them. Right for
 *    `fix-checks`, where success means "the code works now" and a local run
 *    answers that in seconds instead of a CI round trip.
 *  `state` — judge on the world alone. Right for the reconcile rungs, where
 *    success is structural ("no conflict markers left", "no longer behind") and
 *    running tests would answer a *different* question — a merge can be perfectly
 *    correct and still surface a pre-existing test failure, which must not read
 *    as "the merge didn't work".
 *
 *  Rungs default to `state`, the conservative choice: it never invents a failure
 *  the world doesn't show. */
export const RUNG_EVIDENCE: Partial<Record<DelegationKind, "verify" | "state">> = {
  "fix-checks": "verify",
  resolve: "state",
  "update-branch": "state",
  // Judged by the threads, not the tests. A comment round can legitimately change
  // no code at all (answering a question, pushing back), so a test result says
  // nothing about whether it succeeded.
  "resolve-comments": "state",
};

/** How long to wait for evidence once the agent's turn ends. Generous: a CI run
 *  can legitimately take many minutes, and a false "no evidence" wastes a whole
 *  budget slot on a cycle that was actually fine. */
export const EVIDENCE_TIMEOUT_MS = 15 * 60 * 1000;

/** `working` spans the agent's turn; `awaiting-evidence` is the gap between the
 *  turn ending and the world having something to say about it. */
export type CyclePhase = "working" | "awaiting-evidence";

export interface Cycle {
  rung: DelegationKind;
  /** 1-based, compared against `RUNG_BUDGET`. */
  attempt: number;
  /** The observable world at dispatch — see `stateSignature`. */
  signature: string;
  phase: CyclePhase;
  /** Epoch ms the current phase was entered, for the evidence timeout. */
  phaseSince: number;
}

export type StuckReason =
  /** The rung's cycle budget is spent. */
  | "budget-spent"
  /** A cycle ended on a signature that had already produced nothing. */
  | "no-progress"
  /** The ladder wants something autopilot isn't allowed to do. */
  | "needs-human"
  /** The agent pushed back on a review comment and left the thread open. Not a
   *  failure — a disagreement, which only a person can settle. Distinct from
   *  `needs-human` because the next step is reading a specific thread, not
   *  looking at the PR generally. */
  | "disputed-review"
  /** Acting would have swallowed the user's uncommitted work. */
  | "dirty-tree"
  /** No evidence arrived within `EVIDENCE_TIMEOUT_MS`. */
  | "no-evidence";

/** Per-checkout autopilot state, keyed by `checkoutKey`. Absent = never enrolled. */
export interface AutopilotState {
  /** The user turned it on for this checkout. Off by default, everywhere. */
  enrolled: boolean;
  /** Paused by the user; enrollment is kept so resuming is one click. */
  paused: boolean;
  cycle: Cycle | null;
  /** Cycles spent per rung. Reset on a successful cycle, so a long-lived PR
   *  isn't capped globally — only a non-converging stretch is. */
  attempts: Partial<Record<DelegationKind, number>>;
  /** Signatures that have already produced a cycle with no progress. */
  barren: string[];
  /** Set when autopilot handed the checkout back. Sticky: it took a human to get
   *  here, it takes a human to leave, so it can't quietly resume retrying. */
  stuck: { reason: StuckReason; rung: DelegationKind | null; at: number } | null;
}

/** Fresh state for a newly enrolled checkout. */
export function newEnrollment(): AutopilotState {
  return { enrolled: true, paused: false, cycle: null, attempts: {}, barren: [], stuck: null };
}

/** A fingerprint of everything autopilot could act on. Two cycles with the same
 *  signature faced the same world, so whatever happened between them changed
 *  nothing that matters.
 *
 *  Deliberately coarse: the head commit, the sorted failing-check names, and the
 *  sorted conflicted paths. The sha moves whenever the agent commits at all, so a
 *  fix that changed code but not the outcome still counts as progress and earns
 *  another attempt; only a cycle that moved neither code, nor failures, nor the
 *  conflict set reads as barren.
 *
 *  Conflicted paths are in here for the `resolve` rung, whose whole job leaves the
 *  sha untouched until the merge is completed: an attempt that resolved two of
 *  three files made real progress, and without the paths that would look
 *  identical to an attempt that did nothing. Both lists are sorted so a reordered
 *  report isn't mistaken for a change. */
export function stateSignature({ git, checks, comments }: ReadinessInput): string {
  const sha = git?.head_sha ?? "no-head";
  const failing = [...(checks?.required_failing ?? [])].sort().join(",");
  const conflicted = (git?.files ?? [])
    .filter((f) => f.kind === "conflicted")
    .map((f) => f.path)
    .sort()
    .join(",");
  // Thread ids for the same reason as conflicted paths: a comment round often
  // changes no code, so without them a cycle that resolved three of five threads
  // would be indistinguishable from one that did nothing. The `we_replied_last`
  // flag rides along, so a thread turning into a push-back registers as progress
  // too — it moved from "needs us" to "waiting on them".
  const threads = (comments?.unresolved ?? [])
    .map((t) => `${t.id}${t.we_replied_last ? "!" : ""}`)
    .sort()
    .join(",");
  return `${sha}|${failing}|${conflicted}|${threads}`;
}

/** Unstaged, non-conflicted edits in the working copy.
 *
 *  The guard for the `resolve` rung. Its playbook finishes the merge with
 *  `git add -A`, so anything the user was editing gets swept into the merge
 *  commit. Mid-merge the tree legitimately holds the merge's own content — but as
 *  *staged* entries, because git refuses to start a merge with staged changes.
 *  So unstaged non-conflicted edits are exactly the user's in-flight work, and
 *  their presence means this is not autopilot's merge to finish. */
export function unstagedEdits(git: GitState | null): number {
  return (git?.files ?? []).filter((f) => !f.staged && f.kind !== "conflicted").length;
}

export type WaitReason =
  | "not-enrolled"
  | "paused"
  | "stuck"
  | "agent-busy"
  | "delegation-in-flight"
  | "gate-settling"
  | "awaiting-evidence"
  | "nothing-to-do";

/** What autopilot wants done about one checkout this tick. Plain data — the
 *  caller performs the effect and owns the state transition it implies. */
export type AutopilotEffect =
  /** Open a cycle: hand `rung` to the agent, recording `signature` on it. */
  | {
      do: "dispatch";
      rung: DelegationKind;
      action: string;
      params?: Record<string, string>;
      signature: string;
    }
  /** The turn ended: run local verification and enter `awaiting-evidence`. */
  | { do: "verify" }
  /** The turn ended on a state-judged rung: enter `awaiting-evidence` without
   *  running anything. The world is the evidence; it just needs time to settle. */
  | { do: "await-evidence" }
  /** The cycle worked. Clear it and reset the rung's budget. */
  | { do: "settle"; rung: DelegationKind }
  /** The cycle failed but budget remains. Clear it, count the attempt, and
   *  record `barren` (when non-null) so a repeat of that world gives up. */
  | { do: "retry"; rung: DelegationKind; barren: string | null }
  /** Hand back to the human. */
  | { do: "escalate"; reason: StuckReason; rung: DelegationKind | null }
  /** Nothing to do this tick. */
  | { do: "wait"; why: WaitReason };

export interface AutopilotInput {
  state: AutopilotState | undefined;
  readiness: ReadinessInput;
  ladder: LadderContext;
  /** The agent is mid-turn — autopilot never interleaves with a turn it didn't
   *  start. */
  agentBusy: boolean;
  /** A delegation is already in flight for this checkout (possibly the user's). */
  delegationInFlight: boolean;
  /** Local verification since the turn ended, or null when there is none yet —
   *  in which case the cycle is judged on CI alone. */
  verification: VerificationReport | null;
  now: number;
}

/** Decide the next move for one checkout. Pure and total.
 *
 *  Every reason NOT to act is checked before any reason to act, so a paused or
 *  stuck checkout can never be talked into a dispatch by an interesting-looking
 *  ladder result. */
export function autopilotStep(input: AutopilotInput): AutopilotEffect {
  const { state, readiness, ladder, agentBusy, delegationInFlight } = input;

  if (!state?.enrolled) return { do: "wait", why: "not-enrolled" };
  if (state.paused) return { do: "wait", why: "paused" };
  if (state.stuck) return { do: "wait", why: "stuck" };

  if (state.cycle) return judgeCycle(state.cycle, input);

  // ── No cycle in flight: should we start one? ──
  // Never interleave with a turn we didn't start. `delegateAction` would hold the
  // trigger and deliver it once the running turn ends — right for a human click,
  // wrong here: it would append an action to whatever the user just asked for.
  if (agentBusy) return { do: "wait", why: "agent-busy" };
  if (delegationInFlight) return { do: "wait", why: "delegation-in-flight" };

  const rung = nextRung(readiness, ladder);
  switch (rung.do) {
    case "delegate": {
      if (!AUTOPILOT_RUNGS.includes(rung.kind)) {
        // A real action, just not one autopilot may take (a commit; a conflict
        // resolution until that slice lands). The human decides.
        return { do: "escalate", reason: "needs-human", rung: rung.kind };
      }
      // Finishing a merge stages everything (`git add -A`), so unstaged edits
      // alongside the conflict are the user's in-flight work and would be swept
      // into the merge commit. Not autopilot's merge to finish.
      if (rung.kind === "resolve" && unstagedEdits(readiness.git) > 0) {
        return { do: "escalate", reason: "dirty-tree", rung: rung.kind };
      }
      const signature = stateSignature(readiness);
      // Refuse to re-enter a world we already failed to change.
      if (state.barren.includes(signature)) {
        return { do: "escalate", reason: "no-progress", rung: rung.kind };
      }
      if ((state.attempts[rung.kind] ?? 0) >= (RUNG_BUDGET[rung.kind] ?? 0)) {
        return { do: "escalate", reason: "budget-spent", rung: rung.kind };
      }
      return {
        do: "dispatch",
        rung: rung.kind,
        action: rung.action,
        params: rung.params,
        signature,
      };
    }
    case "wait":
      // `gate-computing` / `unknown-state`. Acting on an unsettled world is how a
      // loop convinces itself there's work when there isn't.
      return { do: "wait", why: "gate-settling" };
    case "escalate":
      return {
        do: "escalate",
        // A push-back is its own outcome: the agent did the work and disagreed,
        // so point the user at the argument rather than at the PR in general.
        reason: rung.blocker.kind === "review-disputed" ? "disputed-review" : "needs-human",
        rung: null,
      };
    default:
      // merge / ready / landed. Autopilot's job here is done; merging is a
      // decision, not a remediation, and deliberately not autopilot's to make.
      return { do: "wait", why: "nothing-to-do" };
  }
}

/** Judge a cycle already in flight. Split out so `autopilotStep` reads as the
 *  guard sequence it is. */
function judgeCycle(cycle: Cycle, input: AutopilotInput): AutopilotEffect {
  const { state, readiness, ladder, agentBusy, delegationInFlight, verification, now } = input;

  if (cycle.phase === "working") {
    // Still the agent's turn, or its delegation is still being tracked.
    if (agentBusy || delegationInFlight) return { do: "wait", why: "awaiting-evidence" };
    // The turn ended. A code fix gets a fast local verdict; a reconcile is judged
    // by the world it was meant to change (see `RUNG_EVIDENCE`).
    return RUNG_EVIDENCE[cycle.rung] === "verify" ? { do: "verify" } : { do: "await-evidence" };
  }

  // ── awaiting-evidence ──
  const moved = stateSignature(readiness) !== cycle.signature;
  const barren = moved ? null : cycle.signature;
  const failed = (): AutopilotEffect => {
    // A second barren cycle on the same world means retrying is futile, not
    // unlucky — stop before spending the rest of the budget on it.
    if (barren && (state?.barren.includes(barren) ?? false)) {
      return { do: "escalate", reason: "no-progress", rung: cycle.rung };
    }
    if (cycle.attempt >= (RUNG_BUDGET[cycle.rung] ?? 0)) {
      return { do: "escalate", reason: "budget-spent", rung: cycle.rung };
    }
    return { do: "retry", rung: cycle.rung, barren };
  };

  // Local verification is the cheap, decisive signal for a code fix: if the
  // project's own tests and lints fail, the fix did not work, whatever CI hasn't
  // said yet. Only consulted for rungs judged that way — a reconcile can be
  // correct and still surface a pre-existing failure.
  if (RUNG_EVIDENCE[cycle.rung] === "verify" && verification && !verificationPassed(verification)) {
    return failed();
  }

  const rung = nextRung(readiness, ladder);
  // CI still resolving — no verdict available. Hold, unless we've held so long
  // the cycle is better called inconclusive than successful.
  if (rung.do === "wait") {
    if (now - cycle.phaseSince > EVIDENCE_TIMEOUT_MS) {
      return { do: "escalate", reason: "no-evidence", rung: cycle.rung };
    }
    return { do: "wait", why: "awaiting-evidence" };
  }

  // The world settled. Is the thing we were fixing gone?
  const stillBlocked = rung.do === "delegate" && rung.kind === cycle.rung;
  if (stillBlocked || !moved) return failed();
  return { do: "settle", rung: cycle.rung };
}

/** Whether a verification report is a pass. `skipped` counts as passing — no
 *  command resolved means there was nothing to run, not a failure (mirrors
 *  `VerificationReport::passed` in verify.rs). */
export function verificationPassed(report: VerificationReport): boolean {
  return report.checks.every((c) => c.outcome === "passed" || c.outcome === "skipped");
}

/** The report's failing check names — `"test"` / `"lint"` / `"install"`. This is
 *  the split CI check names can't give us: the local verifier knows which of its
 *  checks is which, where a CI context is just free-form text. */
export function failedCheckNames(report: VerificationReport): string[] {
  return report.checks
    .filter((c) => c.outcome !== "passed" && c.outcome !== "skipped")
    .map((c) => c.name);
}
