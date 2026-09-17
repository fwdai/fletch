// ── Autopilot: move an open PR to the finish line, unattended ────────────────
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
// ── What autopilot is, and isn't ─────────────────────────────────────────────
// Autopilot has one job: once a PR is open — because the user clicked for it or
// asked the agent to — nurse it to mergeable: failing checks, a branch behind or
// conflicting with its base, review comments. Everything else is a no-op. No PR
// yet, uncommitted work, a review gate, a draft, a thread the agent pushed back
// on: autopilot has nothing to do there, and says nothing. Those are not
// "stuck" states and nobody is asked to decide anything — the Git panel's
// primary action already IS the next step, and the user takes it when they
// choose to.
//
// ── Why an unattended loop needs more than a ladder ─────────────────────────
// Every cycle costs an agent turn and a CI run. Two failure modes would burn them
// indefinitely:
//
//   1. A genuinely broken check. The agent "fixes" it, CI fails identically, the
//      ladder says fix-checks again, forever. Caught by the state SIGNATURE: a
//      cycle that ends on the signature it started from changed nothing, and a
//      world already proven barren is never re-entered.
//   2. A rung that oscillates (fix → push → a different failure → fix → the
//      first failure again). Caught by the per-rung attempt BUDGET, which is
//      keyed to the SITUATION (the blocker fingerprint) it was spent on: a
//      different failing check is a different problem and earns its own tries.
//
// Both end in a `give-up` — recorded once, in the checkout's history, as a fact
// about what autopilot did — after which the loop simply waits until the world
// changes. No latch, no state the user has to clear.
//
// Portable to Rust on the same terms as `readiness.ts` — pure, no framework, no
// clock of its own (`now` is a parameter). Enforced by `autopilot.test.ts`.

import type { GitState, VerificationReport } from "@/api";
import type { DelegationKind } from "@/delegation";
import {
  type Blocker,
  detectBlockers,
  type LadderContext,
  nextRung,
  type ReadinessInput,
} from "@/readiness";

/** Rungs autopilot may run on its own.
 *
 *  The commit / push / open-pr rungs are deliberately absent and not planned:
 *  auto-committing someone's working tree is a different risk class from fixing
 *  CI on work they already pushed. The ladder still computes them for the Git
 *  panel's button; autopilot treats them as not its business, which is why an
 *  enrolled checkout with uncommitted edits correctly does nothing. That also
 *  keeps `fix-checks` off a dirty tree — its playbook runs `git add -A`, which
 *  would otherwise sweep the user's in-flight edits into the agent's fix commit. */
export const AUTOPILOT_RUNGS: readonly DelegationKind[] = [
  "fix-checks",
  "resolve",
  "update-branch",
  "resolve-comments",
];

/** Cycles one rung gets on one situation before autopilot gives up on it.
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

/** Why autopilot gave up on a rung. Recorded in the checkout's history and
 *  nowhere else: it is a fact about what autopilot did, not a state the user is
 *  in. Once given up, autopilot waits for the world to change. */
export type GiveUpReason =
  /** The rung's cycle budget for this situation is spent. */
  | "budget-spent"
  /** A cycle ended on a signature that had already produced nothing. */
  | "no-progress"
  /** No evidence arrived within `EVIDENCE_TIMEOUT_MS`. */
  | "no-evidence";

/** Per-checkout autopilot state, keyed by `checkoutKey`. Absent = the driver has
 *  not ticked this checkout yet (or its project has autopilot switched off). */
export interface AutopilotState {
  /** Autopilot is tracking this checkout. On by default: the driver enrolls
   *  every live checkout of a project whose switch is on. */
  enrolled: boolean;
  cycle: Cycle | null;
  /** Cycles spent per rung on the current `situation`. Reset on a successful
   *  cycle, and whenever the situation changes — so a long-lived PR isn't capped
   *  globally, only a non-converging stretch on one problem is. */
  attempts: Partial<Record<DelegationKind, number>>;
  /** The blocker fingerprint `attempts` were spent on (see
   *  `blockerFingerprint`). A dispatch into a different situation starts the
   *  count over: autopilot has not tried and failed at THAT one. Empty until the
   *  first dispatch. */
  situation: string;
  /** Signatures that have already produced a cycle with no progress. Kept
   *  across situations on purpose: a world autopilot has proven it cannot change
   *  stays refused even if the checkout oscillates back to it. */
  barren: string[];
}

/** Fresh state for a newly enrolled checkout. */
export function newEnrollment(): AutopilotState {
  return { enrolled: true, cycle: null, attempts: {}, situation: "", barren: [] };
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

/** A fingerprint of the SITUATION — the blocker kinds plus the detail that
 *  distinguishes one instance from another.
 *
 *  This is what scopes the attempt budget. Autopilot gives up on a problem it
 *  failed to fix three times, and every such problem clears OUTSIDE Fletch — you
 *  push a fix, CI flips, the reviewer approves. When the situation changes, the
 *  budget starts over: the new problem deserves its own tries, and sitting out
 *  would mean abandoning the checkout over a problem that no longer exists.
 *
 *  Deliberately NOT `stateSignature`: that one answers "did the last cycle change
 *  anything" and excludes the merge gate on purpose, because the gate flickers
 *  through `unknown` while CI recomputes and would read as false progress. Here
 *  the question is "is this still the same problem", and a different failing
 *  check or a different conflict set is a different problem. Two questions, two
 *  fingerprints. */
export function blockerFingerprint(blockers: Blocker[]): string {
  return blockers
    .map((b) => {
      switch (b.kind) {
        // Payloads that identify WHICH instance, so a different failure or a
        // different conflict reads as a new situation rather than the old one.
        case "checks-failing":
          return `${b.kind}:${[...b.checks].sort().join(",")}`;
        case "conflicted":
          return `${b.kind}:${[...b.paths].sort().join(",")}`;
        case "review-unaddressed":
        case "review-disputed":
          return `${b.kind}:${b.count}`;
        default:
          return b.kind;
      }
    })
    .sort()
    .join("|");
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

/** Why there is nothing to do this tick. Diagnostics for the driver and the
 *  tests; none of these is shown to the user — a waiting autopilot is the norm,
 *  not news. */
export type WaitReason =
  | "not-enrolled"
  | "agent-busy"
  | "delegation-in-flight"
  /** No open PR: autopilot's job hasn't started. */
  | "no-pr"
  /** The ladder wants something autopilot never does (commit, push, open a PR). */
  | "not-mine"
  /** Finishing the merge would swallow the user's uncommitted edits. */
  | "dirty-tree"
  /** A world autopilot already proved it cannot change. */
  | "no-progress"
  /** Every try for this rung on this situation has been spent. */
  | "budget-spent"
  | "gate-settling"
  | "awaiting-evidence"
  /** Nothing autopilot handles is wrong — mergeable, landed, or a human-owned
   *  gate such as a review. */
  | "nothing-to-do";

/** What autopilot wants done about one checkout this tick. Plain data — the
 *  caller performs the effect and owns the state transition it implies. */
export type AutopilotEffect =
  /** Open a cycle: hand `rung` to the agent, recording `signature` on it and
   *  `situation` on the checkout (a changed situation resets the budget). */
  | {
      do: "dispatch";
      rung: DelegationKind;
      action: string;
      params?: Record<string, string>;
      signature: string;
      situation: string;
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
  /** The cycle failed and autopilot is done with this rung on this situation.
   *  Same bookkeeping as `retry`; the difference is what gets recorded — this is
   *  the one row a returning user is looking for. */
  | { do: "give-up"; rung: DelegationKind; reason: GiveUpReason; barren: string | null }
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

const wait = (why: WaitReason): AutopilotEffect => ({ do: "wait", why });

/** Decide the next move for one checkout. Pure and total.
 *
 *  Every reason NOT to act is checked before any reason to act, so an
 *  interesting-looking ladder result can never talk autopilot into a dispatch
 *  it has no business making. */
export function autopilotStep(input: AutopilotInput): AutopilotEffect {
  const { state, readiness, ladder, agentBusy, delegationInFlight } = input;

  if (!state?.enrolled) return wait("not-enrolled");

  // A cycle in flight is judged whatever the PR does meanwhile — a PR merged or
  // closed under it settles the cycle rather than stranding it.
  if (state.cycle) return judgeCycle(state.cycle, input);

  // ── No cycle in flight: should we start one? ──
  // Autopilot's job begins when a PR is open. Before that — work in progress, a
  // branch not yet proposed — there is nothing here for it.
  if (readiness.pr?.state !== "open") return wait("no-pr");
  // Never interleave with a turn we didn't start. `delegateAction` would hold the
  // trigger and deliver it once the running turn ends — right for a human click,
  // wrong here: it would append an action to whatever the user just asked for.
  if (agentBusy) return wait("agent-busy");
  if (delegationInFlight) return wait("delegation-in-flight");

  const rung = nextRung(readiness, ladder);
  switch (rung.do) {
    case "delegate": {
      // A real action, just not autopilot's (a commit, a push, opening a PR). The
      // Git panel offers it; the user takes it when they choose to.
      if (!AUTOPILOT_RUNGS.includes(rung.kind)) return wait("not-mine");
      // Finishing a merge stages everything (`git add -A`), so unstaged edits
      // alongside the conflict are the user's in-flight work and would be swept
      // into the merge commit. Not autopilot's merge to finish.
      if (rung.kind === "resolve" && unstagedEdits(readiness.git) > 0) return wait("dirty-tree");
      const signature = stateSignature(readiness);
      // Refuse to re-enter a world we already failed to change.
      if (state.barren.includes(signature)) return wait("no-progress");
      // The budget belongs to a situation. A new one starts the count over.
      const situation = blockerFingerprint(detectBlockers(readiness));
      const spent = state.situation === situation ? (state.attempts[rung.kind] ?? 0) : 0;
      if (spent >= (RUNG_BUDGET[rung.kind] ?? 0)) return wait("budget-spent");
      return {
        do: "dispatch",
        rung: rung.kind,
        action: rung.action,
        params: rung.params,
        signature,
        situation,
      };
    }
    case "wait":
      // `gate-computing` / `unknown-state`. Acting on an unsettled world is how a
      // loop convinces itself there's work when there isn't.
      return wait("gate-settling");
    default:
      // escalate / merge / ready / landed. A human-owned gate (a review, a draft,
      // a thread the agent pushed back on) is nobody's remediation, and merging is
      // a decision, not a fix — none of it is autopilot's, so none of it is news.
      return wait("nothing-to-do");
  }
}

/** Judge a cycle already in flight. Split out so `autopilotStep` reads as the
 *  guard sequence it is. */
function judgeCycle(cycle: Cycle, input: AutopilotInput): AutopilotEffect {
  const { state, readiness, ladder, agentBusy, delegationInFlight, verification, now } = input;

  if (cycle.phase === "working") {
    // Still the agent's turn, or its delegation is still being tracked.
    if (agentBusy || delegationInFlight) return wait("awaiting-evidence");
    // The turn ended. A code fix gets a fast local verdict; a reconcile is judged
    // by the world it was meant to change (see `RUNG_EVIDENCE`).
    return RUNG_EVIDENCE[cycle.rung] === "verify" ? { do: "verify" } : { do: "await-evidence" };
  }

  // ── awaiting-evidence ──
  const moved = stateSignature(readiness) !== cycle.signature;
  const barren = moved ? null : cycle.signature;
  const giveUp = (reason: GiveUpReason): AutopilotEffect => ({
    do: "give-up",
    rung: cycle.rung,
    reason,
    barren,
  });
  const failed = (): AutopilotEffect => {
    // A second barren cycle on the same world means retrying is futile, not
    // unlucky — stop before spending the rest of the budget on it.
    if (barren && (state?.barren.includes(barren) ?? false)) return giveUp("no-progress");
    if (cycle.attempt >= (RUNG_BUDGET[cycle.rung] ?? 0)) return giveUp("budget-spent");
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
    if (now - cycle.phaseSince > EVIDENCE_TIMEOUT_MS) return giveUp("no-evidence");
    return wait("awaiting-evidence");
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
