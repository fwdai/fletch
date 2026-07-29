// Autopilot spends agent turns and CI runs without being asked, so the tests
// that matter most are the ones proving it STOPS: on a spent budget, on a world
// it failed to change, on a rung it isn't allowed to take, and on anything the
// user is doing themselves. The happy path is one test; the brakes are twelve.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { CheckOutcome, GitState, PrChecks, PrState, VerificationReport } from "@/api";
import {
  type AutopilotInput,
  type AutopilotState,
  autopilotStep,
  type Cycle,
  EVIDENCE_TIMEOUT_MS,
  failedCheckNames,
  newEnrollment,
  RUNG_BUDGET,
  stateSignature,
  unstagedEdits,
  verificationPassed,
} from "@/autopilot";
import type { LadderContext, ReadinessInput } from "@/readiness";

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
      step({ state: state({ stuck: { reason: "budget-spent", rung: "fix-checks", at: NOW } }) }),
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
    expect(step({ readiness: dirty })).toEqual({
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
    expect(step({ readiness: reviewGate })).toEqual({
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

  it("refuses to re-enter a world it already failed to change", () => {
    // Checked BEFORE the budget: a repeat of a barren world is futile even with
    // attempts to spare.
    const s = state({ barren: [stateSignature(FAILING)] });
    expect(step({ state: s })).toEqual({
      do: "escalate",
      reason: "no-progress",
      rung: "fix-checks",
    });
  });

  it("stops when the rung's budget is spent", () => {
    const s = state({ attempts: { "fix-checks": RUNG_BUDGET["fix-checks"] } });
    expect(step({ state: s })).toEqual({
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
    expect(step({ state: s })).toEqual({
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
    expect(inFlight({ phase: "awaiting-evidence", attempt: RUNG_BUDGET["fix-checks"] })).toEqual({
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
    ).toEqual({ do: "escalate", reason: "no-evidence", rung: "fix-checks" });
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
    expect(step({ readiness: withUserEdit })).toEqual({
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
