// Autopilot spends agent turns and CI runs without being asked, so the tests
// that matter most are the ones proving it STOPS: on a spent budget, on a world
// it failed to change, on a rung it isn't allowed to take, and on anything the
// user is doing themselves. The happy path is one test; the brakes are many.
//
// And the ones proving it stays QUIET: no PR yet, a dirty tree, a review gate, a
// thread the agent pushed back on — none of those is autopilot's business, so
// none of them may produce anything but a wait.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type {
  CheckOutcome,
  GitState,
  PrChecks,
  PrComment,
  PrState,
  VerificationReport,
} from "@/api";
import {
  type AutopilotInput,
  type AutopilotState,
  autopilotStep,
  blockerFingerprint,
  type Cycle,
  EVIDENCE_TIMEOUT_MS,
  failedCheckNames,
  newEnrollment,
  RUNG_BUDGET,
  stateSignature,
  unstagedEdits,
  verificationPassed,
} from "@/autopilot";
import { detectBlockers, type LadderContext, type ReadinessInput } from "@/readiness";

const NOW = 1_000_000;

function git(over: Partial<GitState> = {}): GitState {
  return {
    branch: "feat",
    parent_branch: "main",
    ahead: 1,
    behind: 0,
    unpushed: 0,
    files: [],
    additions: 0,
    deletions: 0,
    has_origin: true,
    head_sha: "sha1",
    ...over,
  };
}
const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 7,
  url: "https://x",
  state: "open",
  title: "t",
  mergeable: "mergeable",
  ...over,
});
const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "clean",
  rollup: "none",
  total: 0,
  passed: 0,
  failed: 0,
  pending: 0,
  required_failing: [],
  runs: [],
  ...over,
});
const report = (...outcomes: [string, CheckOutcome][]): VerificationReport => ({
  checks: outcomes.map(([name, outcome]) => ({
    name,
    command: `run ${name}`,
    outcome,
    duration_ms: 1,
    tail: [],
  })),
});

/** A PR whose required checks are failing — the one world autopilot acts on. */
const FAILING: ReadinessInput = {
  git: git(),
  pr: pr(),
  checks: checks({ merge_state: "blocked", required_failing: ["test"] }),
  comments: { unresolved: [] },
};
/** The same PR, fixed. */
const GREEN: ReadinessInput = {
  git: git({ head_sha: "sha2" }),
  pr: pr(),
  checks: checks({ merge_state: "clean" }),
  comments: { unresolved: [] },
};

const LADDER: LadderContext = { base: "main", commitMode: "commit-pr" };

/** The situation FAILING is — what a dispatch into it stamps on the checkout,
 *  and what spent attempts must carry to count against it. */
const SITUATION = blockerFingerprint(detectBlockers(FAILING));

const state = (over: Partial<AutopilotState> = {}): AutopilotState => ({
  ...newEnrollment(),
  ...over,
});
const cycle = (over: Partial<Cycle> = {}): Cycle => ({
  rung: "fix-checks",
  attempt: 1,
  signature: stateSignature(FAILING),
  phase: "working",
  phaseSince: NOW,
  ...over,
});
/** Attempts spent on FAILING's situation — the only kind that counts. */
const spent = (n: number) => state({ attempts: { "fix-checks": n }, situation: SITUATION });

const step = (over: Partial<AutopilotInput> = {}) =>
  autopilotStep({
    state: state(),
    readiness: FAILING,
    ladder: LADDER,
    agentBusy: false,
    delegationInFlight: false,
    verification: null,
    now: NOW,
    ...over,
  });

describe("autopilot refuses to act", () => {
  it("does nothing at all unless the checkout was enrolled", () => {
    // An absent entry and an un-enrolled one must both be inert.
    expect(step({ state: undefined })).toEqual({ do: "wait", why: "not-enrolled" });
    expect(step({ state: state({ enrolled: false }) })).toEqual({
      do: "wait",
      why: "not-enrolled",
    });
  });

  it("never interleaves with a turn it didn't start", () => {
    // delegateAction would HOLD the trigger and deliver it after the running
    // turn — right for a human click, wrong here: it would append an action to
    // whatever the user just asked for. Skip the tick instead.
    expect(step({ agentBusy: true })).toEqual({ do: "wait", why: "agent-busy" });
    expect(step({ delegationInFlight: true })).toEqual({
      do: "wait",
      why: "delegation-in-flight",
    });
  });

  it("waits out an unsettled world rather than inventing work", () => {
    // A gate GitHub hasn't computed. Acting on it is how a loop convinces itself
    // there is something to do.
    expect(step({ readiness: { ...FAILING, checks: checks({ merge_state: "unknown" }) } })).toEqual(
      { do: "wait", why: "gate-settling" },
    );
  });

  it("does nothing once there is nothing left that it handles", () => {
    // A mergeable PR is not autopilot's to merge — that decision stays the
    // user's, by design.
    expect(step({ readiness: GREEN })).toEqual({ do: "wait", why: "nothing-to-do" });
  });
});

describe("autopilot's job starts when a PR is open, and only then", () => {
  // Before that, the checkout is the user's (or the agent's) work in progress.
  // Nothing here is stuck and nobody is asked to decide anything.

  it("is a no-op with no PR at all, whatever the tree looks like", () => {
    expect(step({ readiness: { ...FAILING, pr: null, git: null } })).toEqual({
      do: "wait",
      why: "no-pr",
    });
    // Pushed, not proposed: opening the PR is the user's move.
    expect(step({ readiness: { ...FAILING, pr: null } })).toEqual({ do: "wait", why: "no-pr" });
    // Uncommitted work: so is committing it.
    const dirty = {
      ...FAILING,
      pr: null,
      git: git({
        files: [{ path: "a.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
      }),
    };
    expect(step({ readiness: dirty })).toEqual({ do: "wait", why: "no-pr" });
  });

  it("is a no-op on a closed or merged PR", () => {
    expect(step({ readiness: { ...FAILING, pr: pr({ state: "closed" }) } })).toEqual({
      do: "wait",
      why: "no-pr",
    });
    expect(step({ readiness: { ...GREEN, pr: pr({ state: "merged" }) } })).toEqual({
      do: "wait",
      why: "no-pr",
    });
  });

  it("leaves a rung it never drives to the user, which is what keeps it off a dirty tree", () => {
    // `fix-checks` runs `git add -A`. The ladder ranks uncommitted work above
    // failing checks, so a dirty tree on an open PR yields a commit rung — not in
    // AUTOPILOT_RUNGS — and autopilot simply has nothing to do, instead of
    // sweeping the user's in-flight edits into an agent commit. Not "stuck": the
    // Git panel's Commit button is the next step, whenever the user wants it.
    const dirty = {
      ...FAILING,
      git: git({
        files: [{ path: "a.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
      }),
    };
    expect(step({ readiness: dirty })).toEqual({ do: "wait", why: "not-mine" });
  });

  it("is a no-op on a human-owned gate", () => {
    // A review gate has no remediation an agent could run. It is also not news:
    // the PR card already says "blocked by a review gate".
    const reviewGate = { ...FAILING, checks: checks({ merge_state: "blocked" }) };
    expect(step({ readiness: reviewGate })).toEqual({ do: "wait", why: "nothing-to-do" });
    const draft = { ...GREEN, checks: checks({ merge_state: "draft" }) };
    expect(step({ readiness: draft })).toEqual({ do: "wait", why: "nothing-to-do" });
  });

  it("still judges a cycle in flight when the PR closes under it, so nothing is stranded", () => {
    // The turn already ran. A PR merged meanwhile settles the cycle rather than
    // leaving it open forever behind the PR gate.
    const merged = { ...GREEN, pr: pr({ state: "merged" }) };
    expect(
      step({ state: state({ cycle: cycle({ phase: "awaiting-evidence" }) }), readiness: merged }),
    ).toEqual({ do: "settle", rung: "fix-checks" });
  });
});

describe("autopilot opens a cycle", () => {
  it("dispatches fix-checks with the failing names, the world and the situation it started from", () => {
    expect(step()).toEqual({
      do: "dispatch",
      rung: "fix-checks",
      action: "fix-checks",
      params: { failing: "test" },
      signature: stateSignature(FAILING),
      situation: SITUATION,
    });
  });

  it("fixes a failing check on a repo with no required checks", () => {
    // The shape that actually reaches production, and the one that sat there doing
    // nothing: no branch-protection required checks (GitHub's default) means a red
    // run reports `unstable`, not `blocked`. Autopilot read that as a merge it
    // wasn't allowed to make and waited forever. `fix-checks` must reach it — the
    // goal is finished work, not work the forge happens to accept.
    const softFailing: ReadinessInput = {
      ...FAILING,
      checks: checks({ merge_state: "unstable", required_failing: ["rust-test"], failed: 1 }),
    };
    expect(step({ readiness: softFailing })).toMatchObject({
      do: "dispatch",
      rung: "fix-checks",
      params: { failing: "rust-test" },
    });
  });

  it("refuses to re-enter a world it already failed to change", () => {
    // Checked BEFORE the budget: a repeat of a barren world is futile even with
    // attempts to spare. Quietly — the give-up that recorded it already said why.
    const s = state({ barren: [stateSignature(FAILING)] });
    expect(step({ state: s })).toEqual({ do: "wait", why: "no-progress" });
  });

  it("waits, quietly, once the rung's budget for this situation is spent", () => {
    expect(step({ state: spent(RUNG_BUDGET["fix-checks"] ?? 0) })).toEqual({
      do: "wait",
      why: "budget-spent",
    });
  });
});

describe("the budget belongs to a situation", () => {
  // Autopilot gives up on a problem it failed to fix three times, and every such
  // problem clears OUTSIDE Fletch — a fix is pushed, CI flips, a reviewer
  // approves. Latching on the checkout would abandon it over a problem that no
  // longer exists, and the next failing check would go unhandled too.

  it("starts the count over when the failing check is a different one", () => {
    // A different failing check is a different problem — autopilot has not tried
    // and failed at this one.
    const otherFailure = {
      ...FAILING,
      checks: checks({ merge_state: "blocked", required_failing: ["lint"] }),
    };
    expect(step({ state: spent(3), readiness: otherFailure })).toMatchObject({
      do: "dispatch",
      rung: "fix-checks",
      situation: blockerFingerprint(detectBlockers(otherFailure)),
    });
  });

  it("does not count attempts that were spent on some other situation", () => {
    const elsewhere = state({ attempts: { "fix-checks": 3 }, situation: "checks-failing:lint" });
    expect(step({ state: elsewhere })).toMatchObject({ do: "dispatch", situation: SITUATION });
  });

  it("stays off a world it already proved barren, whatever the situation says", () => {
    // The safety property behind a fresh budget: `barren` is kept, so an
    // oscillating world (a flaky check flipping back) is refused immediately
    // instead of burning a whole new budget on a world already proven futile.
    const s = state({ barren: [stateSignature(FAILING)], situation: "checks-failing:lint" });
    expect(step({ state: s })).toEqual({ do: "wait", why: "no-progress" });
  });

  it("stamps the dispatch with the situation, so the store can tell a new one from the old", () => {
    expect(step()).toHaveProperty("situation", blockerFingerprint(detectBlockers(FAILING)));
  });
});

describe("autopilot judges a cycle in flight", () => {
  const inFlight = (c: Partial<Cycle>, over: Partial<AutopilotInput> = {}) =>
    step({ state: state({ cycle: cycle(c) }), ...over });

  it("holds while the agent works, then asks for a local verdict", () => {
    expect(inFlight({ phase: "working" }, { agentBusy: true })).toEqual({
      do: "wait",
      why: "awaiting-evidence",
    });
    // Turn over: verify locally rather than waiting minutes for CI to speak.
    expect(inFlight({ phase: "working" })).toEqual({ do: "verify" });
  });

  it("settles when the checks it was fixing are gone and the world moved", () => {
    expect(inFlight({ phase: "awaiting-evidence" }, { readiness: GREEN })).toEqual({
      do: "settle",
      rung: "fix-checks",
    });
  });

  it("believes a failing local verification over CI's silence", () => {
    // The cheap decisive signal: the project's own tests fail, so the fix didn't
    // work — no reason to spend minutes waiting for CI to agree.
    expect(
      inFlight(
        { phase: "awaiting-evidence" },
        { readiness: GREEN, verification: report(["test", "failed"]) },
      ),
    ).toEqual({ do: "retry", rung: "fix-checks", barren: null });
  });

  it("records a barren signature when a cycle changed nothing", () => {
    // Same world as at dispatch → this cycle achieved nothing, so remember the
    // signature. Next time it comes round, autopilot gives up instead.
    expect(inFlight({ phase: "awaiting-evidence" })).toEqual({
      do: "retry",
      rung: "fix-checks",
      barren: stateSignature(FAILING),
    });
  });

  it("gives up the second time the same world produces nothing", () => {
    const s = state({
      cycle: cycle({ phase: "awaiting-evidence" }),
      barren: [stateSignature(FAILING)],
    });
    expect(step({ state: s })).toEqual({
      do: "give-up",
      rung: "fix-checks",
      reason: "no-progress",
      barren: stateSignature(FAILING),
    });
  });

  it("treats a changed commit as progress even when the failure is identical", () => {
    // The agent did change code; the fix just didn't land. That earns another
    // attempt (bounded by the budget) rather than an immediate give-up.
    const movedSha = { ...FAILING, git: git({ head_sha: "sha9" }) };
    expect(inFlight({ phase: "awaiting-evidence" }, { readiness: movedSha })).toEqual({
      do: "retry",
      rung: "fix-checks",
      barren: null,
    });
  });

  it("gives up when the failing cycle was the last one in the budget", () => {
    expect(
      inFlight({ phase: "awaiting-evidence", attempt: RUNG_BUDGET["fix-checks"] }),
    ).toMatchObject({ do: "give-up", reason: "budget-spent", rung: "fix-checks" });
  });

  it("waits for CI, but calls the cycle inconclusive rather than successful if it never speaks", () => {
    const computing = { ...FAILING, checks: checks({ merge_state: "unknown" }) };
    expect(inFlight({ phase: "awaiting-evidence" }, { readiness: computing })).toEqual({
      do: "wait",
      why: "awaiting-evidence",
    });
    // Past the timeout, "no evidence" is the honest verdict — never a silent pass.
    expect(
      inFlight(
        { phase: "awaiting-evidence", phaseSince: NOW - EVIDENCE_TIMEOUT_MS - 1 },
        { readiness: computing },
      ),
    ).toMatchObject({ do: "give-up", reason: "no-evidence", rung: "fix-checks" });
  });

  it("a give-up carries the same barren verdict a retry would, so the world stays refused", () => {
    // Giving up is retry's bookkeeping plus a reason: the store counts the attempt
    // and remembers the barren world, and the next tick finds it barren and waits.
    const given = inFlight({ phase: "awaiting-evidence", attempt: RUNG_BUDGET["fix-checks"] });
    expect(given).toHaveProperty("barren", stateSignature(FAILING));
  });
});

describe("the reconcile rungs", () => {
  /** A mid-merge tree: the merge's own content is STAGED (git refuses to start a
   *  merge with staged changes, so staged entries can only be its output), and the
   *  unresolved files are conflicted. */
  const midMerge = (over: Partial<GitState> = {}): ReadinessInput => ({
    ...FAILING,
    git: git({
      files: [
        { path: "a.ts", kind: "conflicted", staged: false, additions: 1, deletions: 0 },
        { path: "b.ts", kind: "modified", staged: true, additions: 1, deletions: 0 },
      ],
      ...over,
    }),
  });

  it("resolves conflicts, ahead of everything else that's wrong", () => {
    // The PR is also failing checks and behind, but a broken tree comes first —
    // every later rung would build on it.
    expect(step({ readiness: midMerge() })).toMatchObject({
      do: "dispatch",
      rung: "resolve",
      action: "resolve-conflicts",
    });
  });

  it("refuses to finish a merge that would swallow the user's uncommitted work", () => {
    // The playbook completes the merge with `git add -A`. An unstaged,
    // non-conflicted edit is by definition the user's in-flight work (the merge's
    // own content is staged), so this is not autopilot's merge to finish. Quietly:
    // the Git panel's "Resolve" button is the user's, whenever they want it.
    const withUserEdit = midMerge({
      files: [
        { path: "a.ts", kind: "conflicted", staged: false, additions: 1, deletions: 0 },
        { path: "mine.ts", kind: "modified", staged: false, additions: 9, deletions: 0 },
      ],
    });
    expect(step({ readiness: withUserEdit })).toEqual({ do: "wait", why: "dirty-tree" });
  });

  it("leaves local conflicts alone when no PR is open", () => {
    expect(step({ readiness: { ...midMerge(), pr: null } })).toEqual({ do: "wait", why: "no-pr" });
  });

  it("updates a branch that has fallen behind its base", () => {
    const behind = {
      ...FAILING,
      checks: checks({ merge_state: "behind", required_failing: ["test"] }),
    };
    expect(step({ readiness: behind })).toMatchObject({
      do: "dispatch",
      rung: "update-branch",
      params: { base: "main" },
    });
  });

  it("judges a reconcile on the world, not by running the tests", () => {
    // A merge can be perfectly correct and still surface a pre-existing test
    // failure. Verifying would answer a different question, so these rungs skip it.
    for (const rung of ["resolve", "update-branch"] as const) {
      expect(step({ state: state({ cycle: cycle({ rung, phase: "working" }) }) })).toEqual({
        do: "await-evidence",
      });
    }
    // And a failing local report must not condemn one.
    expect(
      step({
        state: state({ cycle: cycle({ rung: "update-branch", phase: "awaiting-evidence" }) }),
        readiness: GREEN,
        verification: report(["test", "failed"]),
      }),
    ).toEqual({ do: "settle", rung: "update-branch" });
  });

  it("still runs the tests for a code fix", () => {
    expect(step({ state: state({ cycle: cycle({ phase: "working" }) }) })).toEqual({
      do: "verify",
    });
  });

  it("gives a reconcile two attempts, not three", () => {
    for (const rung of ["resolve", "update-branch"] as const) {
      expect(RUNG_BUDGET[rung]).toBe(2);
    }
  });
});

describe("the review-comments rung", () => {
  const thread = (id: string, over: Partial<PrComment> = {}): PrComment => ({
    id,
    author: "greptileai",
    is_bot: true,
    body: "Consider the null case",
    path: "a.ts",
    line: 1,
    url: "https://x",
    replies: 0,
    we_replied_last: false,
    ...over,
  });
  const withThreads = (...t: PrComment[]): ReadinessInput => ({
    ...GREEN,
    comments: { unresolved: t },
  });

  it("works threads that are waiting on us, with the count", () => {
    expect(step({ readiness: withThreads(thread("t1"), thread("t2")) })).toMatchObject({
      do: "dispatch",
      rung: "resolve-comments",
      params: { count: "2" },
    });
  });

  it("never re-argues a thread it already pushed back on", () => {
    // We replied last and left it open on purpose. Re-dispatching would post a
    // duplicate reply into a real person's conversation every cycle. And it is
    // not news either: the disagreement is right there in the PR's threads.
    expect(step({ readiness: withThreads(thread("t1", { we_replied_last: true })) })).toEqual({
      do: "wait",
      why: "nothing-to-do",
    });
  });

  it("engages again once the human answers", () => {
    // `we_replied_last` flips false when they reply after us — new input, our turn.
    expect(step({ readiness: withThreads(thread("t1", { replies: 2 })) })).toMatchObject({
      do: "dispatch",
      rung: "resolve-comments",
    });
  });

  it("is judged by the threads, not by the tests", () => {
    // A comment round can legitimately change no code — answering a question,
    // pushing back — so a failing test result says nothing about whether it worked.
    expect(
      step({ state: state({ cycle: cycle({ rung: "resolve-comments", phase: "working" }) }) }),
    ).toEqual({ do: "await-evidence" });
    expect(
      step({
        state: state({ cycle: cycle({ rung: "resolve-comments", phase: "awaiting-evidence" }) }),
        readiness: GREEN,
        verification: report(["test", "failed"]),
      }),
    ).toEqual({ do: "settle", rung: "resolve-comments" });
  });

  it("counts a push-back as progress, not as a barren cycle", () => {
    // The thread went from "needs us" to "waiting on them". No code moved and the
    // thread is still open, so without the signature tracking that flag this would
    // look like a cycle that achieved nothing.
    const before = withThreads(thread("t1"));
    const after = withThreads(thread("t1", { we_replied_last: true }));
    expect(stateSignature(before)).not.toBe(stateSignature(after));
  });

  it("counts a partial round as progress", () => {
    // Three threads down to one: real work, even though the sha may not have moved.
    const before = withThreads(thread("t1"), thread("t2"), thread("t3"));
    const after = withThreads(thread("t3"));
    expect(stateSignature(before)).not.toBe(stateSignature(after));
  });

  it("gets two attempts — each one posts into a real conversation", () => {
    expect(RUNG_BUDGET["resolve-comments"]).toBe(2);
  });
});

describe("unstagedEdits", () => {
  it("counts only the user's in-flight work, not the merge's own content", () => {
    expect(unstagedEdits(null)).toBe(0);
    expect(
      unstagedEdits(
        git({
          files: [
            // The conflict itself: autopilot's to resolve.
            { path: "a.ts", kind: "conflicted", staged: false, additions: 1, deletions: 0 },
            // Cleanly merged by git, already staged: also the merge's.
            { path: "b.ts", kind: "modified", staged: true, additions: 1, deletions: 0 },
          ],
        }),
      ),
    ).toBe(0);
    expect(
      unstagedEdits(
        git({
          files: [{ path: "mine.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
        }),
      ),
    ).toBe(1);
  });
});

describe("stateSignature", () => {
  it("changes with the commit and with the set of failures", () => {
    expect(stateSignature(FAILING)).not.toBe(stateSignature(GREEN));
    expect(stateSignature(FAILING)).not.toBe(
      stateSignature({ ...FAILING, git: git({ head_sha: "other" }) }),
    );
  });

  it("ignores the order CI reports its checks in", () => {
    const a = { ...FAILING, checks: checks({ required_failing: ["lint", "test"] }) };
    const b = { ...FAILING, checks: checks({ required_failing: ["test", "lint"] }) };
    expect(stateSignature(a)).toBe(stateSignature(b));
  });

  it("is stable when nothing observable changed", () => {
    expect(stateSignature(FAILING)).toBe(stateSignature({ ...FAILING }));
  });

  it("tracks the conflict set, so a partial resolution counts as progress", () => {
    // `resolve` leaves the sha untouched until the merge is completed, so without
    // the conflicted paths an attempt that fixed two of three files would look
    // identical to one that did nothing — and be written off as barren.
    const conflicts = (...paths: string[]): ReadinessInput => ({
      ...FAILING,
      git: git({
        files: paths.map((path) => ({
          path,
          kind: "conflicted" as const,
          staged: false,
          additions: 1,
          deletions: 0,
        })),
      }),
    });
    expect(stateSignature(conflicts("a.ts", "b.ts"))).not.toBe(stateSignature(conflicts("a.ts")));
    // Order of the report doesn't matter.
    expect(stateSignature(conflicts("a.ts", "b.ts"))).toBe(
      stateSignature(conflicts("b.ts", "a.ts")),
    );
  });
});

describe("blockerFingerprint", () => {
  it("distinguishes which instance of a blocker, not just its kind", () => {
    const fp = (names: string[]) =>
      blockerFingerprint(
        detectBlockers({
          ...FAILING,
          checks: checks({ merge_state: "blocked", required_failing: names }),
        }),
      );
    expect(fp(["test"])).not.toBe(fp(["lint"]));
    // Order of CI's report is not a change.
    expect(fp(["test", "lint"])).toBe(fp(["lint", "test"]));
  });

  it("is empty when nothing is blocking", () => {
    expect(blockerFingerprint(detectBlockers(GREEN))).toBe("");
    expect(blockerFingerprint(detectBlockers(FAILING))).not.toBe("");
  });
});

describe("verification verdicts", () => {
  it("counts skipped as passing — nothing to run is not a failure", () => {
    expect(verificationPassed(report(["test", "passed"], ["lint", "skipped"]))).toBe(true);
    expect(verificationPassed(report())).toBe(true);
  });

  it("counts every non-pass as a failure, including a blocked setup", () => {
    for (const outcome of ["failed", "timed_out", "setup_failed"] as const) {
      expect(verificationPassed(report(["test", outcome]))).toBe(false);
    }
  });

  it("names the failures, which is the tests-vs-lint split CI can't give us", () => {
    // A CI context is free-form text; the local verifier's checks are named.
    expect(
      failedCheckNames(report(["install", "passed"], ["test", "failed"], ["lint", "failed"])),
    ).toEqual(["test", "lint"]);
  });
});

describe("portability to Rust", () => {
  const source = readFileSync(fileURLToPath(new URL("./autopilot.ts", import.meta.url)), "utf8");
  const imports = [...source.matchAll(/^import\s[\s\S]*?from\s+"([^"]+)";$/gm)].map((m) => m[1]);
  // Assert against code, not prose — the header documents these rules, and naming
  // a banned construct in order to ban it must not trip the guard.
  const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

  it("imports only from the pure core", () => {
    expect(imports.sort()).toEqual(["@/api", "@/delegation", "@/readiness"]);
  });

  it("pulls in no framework, store, or platform runtime", () => {
    for (const forbidden of ["react", "zustand", "@tauri-apps", "@/store", "@/components"]) {
      expect(imports.filter((i) => i.includes(forbidden))).toEqual([]);
    }
  });

  it("reads no clock or randomness, so a pass is reproducible from its inputs", () => {
    expect(code).not.toMatch(/Date\.now|new Date|Math\.random/);
  });
});
