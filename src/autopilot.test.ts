// Autopilot spends agent turns and CI runs without being asked, so the tests
// that matter most are the ones proving it STOPS: on a spent budget, on a world
// it failed to change, on a rung it isn't allowed to take, and on anything the
// user is doing themselves. The happy path is one test; the brakes are twelve.

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
  type StuckReason,
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

/** The blocker fingerprint of FAILING — the situation a stuck fixture stopped in.
 *  Stamping it means "stays stuck" tests assert the world is UNCHANGED, which is
 *  the actual precondition for staying stopped. */
const STUCK_ON = blockerFingerprint(detectBlockers(FAILING));

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
  it("does nothing at all unless the checkout was explicitly enrolled", () => {
    // Default-off everywhere is the whole safety posture; an absent entry and an
    // un-enrolled one must both be inert.
    expect(step({ state: undefined })).toEqual({ do: "wait", why: "not-enrolled" });
    expect(step({ state: state({ enrolled: false }) })).toEqual({
      do: "wait",
      why: "not-enrolled",
    });
  });

  it("stays paused, and stays stuck, however inviting the ladder looks", () => {
    // Both checks sit ahead of every reason to act, so a failing PR can't talk a
    // paused or handed-back checkout into another dispatch.
    expect(step({ state: state({ paused: true }) })).toEqual({ do: "wait", why: "paused" });
    expect(
      step({
        state: state({
          stuck: { reason: "budget-spent", rung: "fix-checks", at: NOW, blockers: STUCK_ON },
        }),
      }),
    ).toEqual({ do: "wait", why: "stuck" });
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

  it("escalates a rung it isn't allowed to take, which is what keeps it off a dirty tree", () => {
    // `fix-checks` runs `git add -A`. The ladder ranks uncommitted work above
    // failing checks, so a dirty tree yields a commit rung — not in
    // AUTOPILOT_RUNGS — and autopilot hands it back instead of sweeping the
    // user's in-flight edits into an agent commit.
    const dirty = {
      ...FAILING,
      git: git({
        files: [{ path: "a.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
      }),
    };
    expect(step({ readiness: dirty })).toMatchObject({
      do: "escalate",
      reason: "needs-human",
      rung: "commit-push",
    });
  });

  it("waits out an unsettled world rather than inventing work", () => {
    // No read yet, and a gate GitHub hasn't computed. Acting on either is how a
    // loop convinces itself there is something to do.
    expect(step({ readiness: { ...FAILING, git: null } })).toEqual({
      do: "wait",
      why: "gate-settling",
    });
    expect(step({ readiness: { ...FAILING, checks: checks({ merge_state: "unknown" }) } })).toEqual(
      { do: "wait", why: "gate-settling" },
    );
  });

  it("escalates a human-only gate instead of retrying it", () => {
    const reviewGate = { ...FAILING, checks: checks({ merge_state: "blocked" }) };
    expect(step({ readiness: reviewGate })).toMatchObject({
      do: "escalate",
      reason: "needs-human",
      rung: null,
    });
  });

  it("does nothing once there is nothing left that it handles", () => {
    // A mergeable PR is not autopilot's to merge — that decision stays the
    // user's, this slice and by design.
    expect(step({ readiness: GREEN })).toEqual({ do: "wait", why: "nothing-to-do" });
  });
});

describe("autopilot opens a cycle", () => {
  it("dispatches fix-checks with the failing names and the world it started from", () => {
    expect(step()).toEqual({
      do: "dispatch",
      rung: "fix-checks",
      action: "fix-checks",
      params: { failing: "test" },
      signature: stateSignature(FAILING),
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
    // attempts to spare.
    const s = state({ barren: [stateSignature(FAILING)] });
    expect(step({ state: s })).toMatchObject({
      do: "escalate",
      reason: "no-progress",
      rung: "fix-checks",
    });
  });

  it("stops when the rung's budget is spent", () => {
    const s = state({ attempts: { "fix-checks": RUNG_BUDGET["fix-checks"] } });
    expect(step({ state: s })).toMatchObject({
      do: "escalate",
      reason: "budget-spent",
      rung: "fix-checks",
    });
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
    expect(step({ state: s })).toMatchObject({
      do: "escalate",
      reason: "no-progress",
      rung: "fix-checks",
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
    ).toMatchObject({
      do: "escalate",
      reason: "budget-spent",
      rung: "fix-checks",
    });
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
    ).toMatchObject({ do: "escalate", reason: "no-evidence", rung: "fix-checks" });
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
    // own content is staged), so this is not autopilot's merge to finish.
    const withUserEdit = midMerge({
      files: [
        { path: "a.ts", kind: "conflicted", staged: false, additions: 1, deletions: 0 },
        { path: "mine.ts", kind: "modified", staged: false, additions: 9, deletions: 0 },
      ],
    });
    expect(step({ readiness: withUserEdit })).toMatchObject({
      do: "escalate",
      reason: "dirty-tree",
      rung: "resolve",
    });
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

  it("never re-argues a thread it already pushed back on, and says so specifically", () => {
    // We replied last and left it open on purpose. Re-dispatching would post a
    // duplicate reply into a real person's conversation every cycle. The reason is
    // its own kind: the next step is reading that thread, not eyeballing the PR.
    expect(step({ readiness: withThreads(thread("t1", { we_replied_last: true })) })).toMatchObject(
      {
        do: "escalate",
        reason: "disputed-review",
        rung: null,
      },
    );
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

describe("autopilot stops only while it is genuinely blocked", () => {
  // It stops because it needs the user — a dirty tree it won't commit over, a
  // review gate, a thread it pushed back on, a fix it failed to land. Every one
  // of those clears OUTSIDE Fletch. Latching on the reason alone meant autopilot
  // never noticed and abandoned the checkout for good, so the next failing check
  // went unhandled too.

  const stuckOn = (readiness: ReadinessInput, reason: StuckReason = "budget-spent") =>
    state({
      stuck: {
        reason,
        rung: "fix-checks",
        at: NOW,
        blockers: blockerFingerprint(detectBlockers(readiness)),
      },
    });

  it("stays stopped while the same thing is still blocking it", () => {
    expect(step({ state: stuckOn(FAILING), readiness: FAILING })).toEqual({
      do: "wait",
      why: "stuck",
    });
  });

  it("picks the checkout back up once that thing is gone", () => {
    // The user fixed the failing check themselves. Sitting out now would mean
    // abandoning the checkout over a problem that no longer exists.
    expect(step({ state: stuckOn(FAILING), readiness: GREEN })).toEqual({ do: "revive" });
  });

  it("picks it back up when the blocker changes rather than clears", () => {
    // A different failing check is a different situation — autopilot has not
    // tried and failed at this one.
    const otherFailure = {
      ...FAILING,
      checks: checks({ merge_state: "blocked", required_failing: ["lint"] }),
    };
    expect(step({ state: stuckOn(FAILING), readiness: otherFailure })).toEqual({ do: "revive" });
  });

  it("resumes after the user commits the work it refused to commit over", () => {
    // The dirty-tree stop, which is the case whose copy most clearly implies
    // autopilot will carry on once you have committed.
    const dirty = {
      ...FAILING,
      git: git({
        files: [{ path: "a.ts", kind: "modified", staged: false, additions: 1, deletions: 0 }],
      }),
    };
    expect(step({ state: stuckOn(dirty, "dirty-tree"), readiness: dirty })).toEqual({
      do: "wait",
      why: "stuck",
    });
    // Committed: the tree is clean and the checks are the only thing left.
    expect(step({ state: stuckOn(dirty, "dirty-tree"), readiness: FAILING })).toEqual({
      do: "revive",
    });
  });

  it("resumes after a reviewer approves, which moves nothing but the gate", () => {
    // The case a state-signature comparison would MISS: an approval changes no
    // commit, no check and no thread. Comparing blockers is what catches it.
    const gated = { ...FAILING, checks: checks({ merge_state: "blocked" }) };
    expect(step({ state: stuckOn(gated, "needs-human"), readiness: gated })).toEqual({
      do: "wait",
      why: "stuck",
    });
    expect(step({ state: stuckOn(gated, "needs-human"), readiness: GREEN })).toEqual({
      do: "revive",
    });
  });

  it("resumes after the user settles a thread it pushed back on", () => {
    const disputed: ReadinessInput = {
      ...GREEN,
      comments: {
        unresolved: [
          {
            id: "t1",
            author: "alice",
            is_bot: false,
            body: "no",
            path: null,
            line: null,
            url: "https://x",
            replies: 2,
            we_replied_last: true,
          },
        ],
      },
    };
    expect(step({ state: stuckOn(disputed, "disputed-review"), readiness: disputed })).toEqual({
      do: "wait",
      why: "stuck",
    });
    // Thread resolved on GitHub → it drops out of `unresolved` entirely.
    expect(step({ state: stuckOn(disputed, "disputed-review"), readiness: GREEN })).toEqual({
      do: "revive",
    });
  });

  it("won't spend its refreshed budget re-entering a world it already failed at", () => {
    // The safety property behind revive granting fresh attempts: `barren` is kept,
    // so an oscillating world (a flaky check flipping back) escalates immediately
    // instead of burning a whole new budget on a world already proven futile.
    const revived = state({ attempts: {}, barren: [stateSignature(FAILING)] });
    expect(step({ state: revived, readiness: FAILING })).toMatchObject({
      do: "escalate",
      reason: "no-progress",
    });
  });

  it("stamps every escalation with the situation it stopped in", () => {
    // Without this the guard above has nothing to compare and autopilot could
    // never tell a cleared blocker from a persisting one.
    const escalation = step({ state: state({ attempts: { "fix-checks": 3 } }) });
    expect(escalation).toMatchObject({ do: "escalate" });
    expect(escalation).toHaveProperty("blockers", blockerFingerprint(detectBlockers(FAILING)));
  });

  it("still refuses to act while paused, whatever the world does", () => {
    // Paused is the user's explicit instruction, so unlike `stuck` it is NOT
    // conditional on anything.
    expect(step({ state: state({ paused: true }), readiness: GREEN })).toEqual({
      do: "wait",
      why: "paused",
    });
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

  it("is empty when nothing is blocking, so a cleared world never matches", () => {
    expect(blockerFingerprint(detectBlockers(GREEN))).toBe("");
    expect(blockerFingerprint(detectBlockers(FAILING))).not.toBe("");
  });
});
