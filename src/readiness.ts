// ── Readiness: what stands between this work and landing, and what fixes it ──
//
// Two pure functions, and they are the single source of truth for the question
// every surface asks in its own words:
//
//   detectBlockers  — WHAT is wrong.  (one forge-shaped function, see below)
//   nextRung        — WHAT TO DO about the most blocking thing.
//
// The Git panel used to answer both inline, and Mission Control's `approveAgent`
// answered them again independently. Two copies of one ladder is how the two
// surfaces came to disagree about what "checks failing" even counts (see
// `checksFailed` below). One classification, many renderings.
//
// ── Deliberately portable to Rust ────────────────────────────────────────────
// The plan is for this loop to eventually run in the supervisor rather than the
// webview, because frontend polling stops when the window is hidden
// (`usePoll` clears its interval on `document.hidden`) — so autopilot pauses
// exactly when nobody is watching. To keep that move mechanical rather than a
// rewrite, this module obeys these rules, enforced by `readiness.test.ts`:
//
//   1. Imports nothing but types from `@/api` and its sibling pure modules. No
//      React, no store, no Tauri, no IO.
//   2. Every input is a serde mirror of a struct Rust already builds
//      (`GitState` <- git_state.rs, `PrChecks`/`PrComments` <- github/*.rs), so
//      the Rust version reads its own types rather than re-deriving them.
//   3. Output is plain data — discriminated unions that map onto Rust enums, no
//      closures, no classes, no `Date.now()`. Callers do the side effects.
//
// ── What is and isn't forge-coupled ──────────────────────────────────────────
// `Blocker` names a *problem*, not a git operation: "tests failing", "review
// unaddressed", "diverged from mainline". `detectBlockers` is the one function
// that knows those problems are currently read out of git and GitHub. A second
// SCM (or none) means a second detector; the taxonomy, the ordering, the
// remediation mapping and everything downstream stay put.

import type { GitState, PrChecks, PrComments, PrState } from "@/api";
import type { DelegationKind } from "@/delegation";
import { describeMergeGate } from "@/mergeGate";

/** A reason this work isn't ready to land. Ordered by the ladder, not by this
 *  declaration — see `nextRung`. */
export type Blocker =
  /** The working copy has unresolved conflict markers. */
  | { kind: "conflicted"; paths: string[] }
  /** Edits in the working copy that were never committed. */
  | { kind: "uncommitted"; files: number }
  /** Commits that exist only locally. */
  | { kind: "unpushed"; commits: number }
  /** Pushed, but never proposed for review. */
  | { kind: "unsubmitted" }
  /** Mainline moved; this can't merge cleanly as it stands. */
  | { kind: "diverged"; mainline: string }
  /** Failing checks. Carries their names — CI can't currently tell us which are
   *  *required* (that needs an API call the app token often can't make), so this
   *  is every failing check, and it is deliberately NOT split into tests-vs-lint:
   *  check names are free-form, and guessing from them would be a heuristic
   *  masquerading as a fact. The local verifier (`verify.rs`) *does* know the
   *  difference — when its report becomes an input, the split can be real. */
  | { kind: "checks-failing"; checks: string[] }
  /** Review threads nobody has addressed. */
  | { kind: "review-unaddressed"; count: number }
  /** A human has to approve. Not agent-fixable — the ladder escalates. */
  | { kind: "review-required" }
  /** Still a draft, so it isn't really proposed yet. Human-owned. */
  | { kind: "draft" }
  /** The proposal was closed without landing. Reopening or replacing it is a
   *  human call, so this escalates rather than silently reading as ready. */
  | { kind: "proposal-closed" };

export interface ReadinessInput {
  /** Null while the first read is in flight — reported as `unknown`, never as
   *  "nothing wrong". */
  git: GitState | null;
  /** Null when this checkout has no proposal open. */
  pr: PrState | null;
  /** Null when the checks read didn't resolve — distinct from a rollup of zero
   *  checks, so a transient failure never reads as "all green". */
  checks: PrChecks | null;
  /** Null when review threads haven't been read for this checkout. */
  comments: PrComments | null;
}

/** Everything standing between this work and landing, most blocking first.
 *
 *  The one forge-coupled function in the module: it knows blockers are read out
 *  of `git status`, GitHub's merge gate and review threads. Empty means nothing
 *  is in the way — which is NOT the same as "landed" or "still loading"; ask
 *  `nextRung` for that distinction. */
export function detectBlockers({ git, pr, checks, comments }: ReadinessInput): Blocker[] {
  if (!git) return [];
  const blockers: Blocker[] = [];

  const conflicted = git.files.filter((f) => f.kind === "conflicted").map((f) => f.path);
  if (conflicted.length > 0) blockers.push({ kind: "conflicted", paths: conflicted });
  else if (git.files.length > 0) {
    // Only when nothing is conflicted: mid-conflict, "uncommitted" is a
    // restatement of the conflict, not a second thing to fix.
    blockers.push({ kind: "uncommitted", files: git.files.length });
  }

  if (git.unpushed > 0) blockers.push({ kind: "unpushed", commits: git.unpushed });

  const prOpen = pr?.state === "open";
  // A merged or closed proposal is not "unsubmitted" — there is nothing to
  // propose. Only work that exists and was never proposed counts.
  if (!pr && git.ahead > 0) blockers.push({ kind: "unsubmitted" });
  if (pr?.state === "closed") blockers.push({ kind: "proposal-closed" });

  if (prOpen) {
    // Gate semantics live in one place (`describeMergeGate`); this maps its
    // verdict onto the blocker taxonomy. `required_failing` — not `checks.failed`
    // — is what the gate's `checksFailed` means, and disagreeing about that is
    // the exact drift this module exists to end.
    const failing = checks?.required_failing ?? [];
    const gate = describeMergeGate(checks?.merge_state ?? null, {
      checksFailed: failing.length,
      mergeable: pr.mergeable,
    });
    switch (gate.situation) {
      case "conflicts":
      case "behind":
        blockers.push({ kind: "diverged", mainline: git.parent_branch });
        break;
      case "checks-failing":
        blockers.push({ kind: "checks-failing", checks: failing });
        break;
      case "review-required":
        blockers.push({ kind: "review-required" });
        break;
      case "draft":
        blockers.push({ kind: "draft" });
        break;
      default:
        // ready / mergeable-soft / no-conflicts / computing add no blocker of
        // their own. `mergeable-soft` means only non-required checks fail, so
        // the failing names are informational, not blocking.
        break;
    }
    const unresolved = comments?.unresolved.length ?? 0;
    if (unresolved > 0) blockers.push({ kind: "review-unaddressed", count: unresolved });
  }

  return blockers;
}

/** How the ladder wants the most blocking problem handled. Plain data: the
 *  caller performs the effect and owns any scoping (e.g. adding `repo="<subdir>"`
 *  to a multi-repo trigger). */
export type Rung =
  /** Hand it to the agent. `action`/`params` name the playbook trigger. */
  | {
      do: "delegate";
      kind: DelegationKind;
      action: string;
      params?: Record<string, string>;
      blocker: Blocker;
    }
  /** Nothing left in the way and the forge's gate is open. */
  | { do: "merge" }
  /** Blocked on something only a human can clear. */
  | { do: "escalate"; blocker: Blocker }
  /** Don't act: the state isn't settled enough to trust. */
  | { do: "wait"; why: "unknown-state" | "gate-computing" }
  /** Already landed. */
  | { do: "landed" }
  /** Nothing blocking, but the gate isn't open (or says nothing) — merging is a
   *  decision, not a remediation, so the caller chooses. */
  | { do: "ready" };

export interface LadderContext {
  /** Mainline branch, for the actions that name it. */
  base: string;
  /** The user's sticky commit mode, as a neutral triple so this module needs no
   *  import from the panel. */
  commitMode: "commit" | "commit-push" | "commit-pr";
}

/** The remediation ladder: the ONE ordering of what to do next.
 *
 *  Ordered most-blocking first, so each rung's work isn't wasted by the next:
 *  reconcile the working copy before committing it, get the work published
 *  before asking the forge about it, sync with mainline before trusting a check
 *  result, fix the checks before reading review threads written against them.
 *
 *  Pure and total — every state yields a rung, so no caller has to invent a
 *  fallback (which is how the two old copies diverged). */
export function nextRung(input: ReadinessInput, ctx: LadderContext): Rung {
  const { git, pr, checks } = input;
  // No read yet: acting on absent state would be acting on a guess.
  if (!git) return { do: "wait", why: "unknown-state" };
  if (pr?.state === "merged") return { do: "landed" };

  const blockers = detectBlockers(input);
  const find = <K extends Blocker["kind"]>(kind: K) =>
    blockers.find((b): b is Extract<Blocker, { kind: K }> => b.kind === kind);

  const conflicted = find("conflicted");
  if (conflicted) {
    return {
      do: "delegate",
      kind: "resolve",
      action: "resolve-conflicts",
      blocker: conflicted,
    };
  }

  const uncommitted = find("uncommitted");
  if (uncommitted) {
    // With a proposal already open, "open a PR" degrades to "push" — that is
    // what updates the existing one.
    const mode =
      ctx.commitMode === "commit-pr" && pr?.state === "open" ? "commit-push" : ctx.commitMode;
    return {
      do: "delegate",
      kind: mode,
      action: mode,
      params: mode === "commit-pr" ? { base: ctx.base } : undefined,
      blocker: uncommitted,
    };
  }

  const unsubmitted = find("unsubmitted");
  if (unsubmitted) {
    return {
      do: "delegate",
      kind: "open-pr",
      action: "open-pr",
      params: { base: ctx.base },
      blocker: unsubmitted,
    };
  }

  const unpushed = find("unpushed");
  if (unpushed) {
    return { do: "delegate", kind: "push", action: "push", blocker: unpushed };
  }

  const diverged = find("diverged");
  if (diverged) {
    return {
      do: "delegate",
      kind: "update-branch",
      action: "update-branch",
      params: { base: ctx.base },
      blocker: diverged,
    };
  }

  const failing = find("checks-failing");
  if (failing) {
    return {
      do: "delegate",
      kind: "fix-checks",
      action: "fix-checks",
      params: { failing: failing.checks.join(", ") },
      blocker: failing,
    };
  }

  // Human-owned gates: no remediation exists that the agent could run.
  const human = find("review-required") ?? find("draft") ?? find("proposal-closed");
  if (human) return { do: "escalate", blocker: human };

  // No rung addresses review threads yet — the delegation kind and its playbook
  // arrive with the comments slice. Escalating (rather than silently treating a
  // commented PR as ready) keeps the honest answer: a human should look.
  const comments = find("review-unaddressed");
  if (comments) return { do: "escalate", blocker: comments };

  // Only an open proposal HAS a merge gate. Consulting it otherwise reads
  // GitHub's "not computed yet" as "still resolving" and parks forever on a
  // checkout that simply has no PR — nothing to merge, nothing to wait for.
  if (pr?.state !== "open") return { do: "ready" };

  // Nothing blocking. Whether that means "merge" depends on the forge's gate,
  // which can still be computing — never merge off a gate that hasn't settled.
  const gate = describeMergeGate(checks?.merge_state ?? null, {
    checksFailed: checks?.required_failing.length ?? 0,
    mergeable: pr.mergeable,
  });
  if (gate.situation === "computing") return { do: "wait", why: "gate-computing" };
  // Every gate that forbids merging has already surfaced as a blocker above, so
  // this is a defensive total-function fallback, not an expected path.
  return gate.mergeAllowed ? { do: "merge" } : { do: "ready" };
}
