// The autopilot POLICY is pure and exhaustively covered in `autopilot.test.ts`.
// What is left is the wiring — the store transitions, the delegation the effect
// turns into, and the two guards that only exist in the pass itself:
//
//   1. One dispatch per agent per pass. `delegateAction` sends the agent a
//      message, but the status flip to `running` comes back asynchronously from
//      the backend, so both checkouts of a multi-repo agent see an `idle` agent
//      inside a single pass. Both dispatching coalesces two triggers into one
//      turn — what `queued` exists to prevent — and `delegationInFlight` can't
//      see it because it is keyed per checkout.
//   2. One verification per checkout in flight. A verify outlives a pass, and a
//      dependency change re-runs `usePoll`'s effect (whose own `inFlight` flag is
//      fresh), so a slow verify can be re-entered; the caller's `verifying` set
//      is what survives that.
//
// `autopilotPass` is the whole sweep, so both are testable without a rendered
// hook — the same reason `planDelegationPass` is exported.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";
import type { AgentStatus, GitState, PrChecks, PrState, VerificationReport } from "@/api";
import type { AutopilotState, Cycle } from "@/autopilot";
import { newEnrollment } from "@/autopilot";

const { runVerification } = vi.hoisted(() => ({ runVerification: vi.fn() }));
vi.mock("@/api", () => ({ api: { runVerification } }));
vi.mock("@/storage/settings", () => ({ setSetting: vi.fn() }));

// The pass reads `useAppStore.getState()` on every key, so the mock has to
// forward to whatever store the current test built.
const held = vi.hoisted(() => ({ store: null as { getState: () => unknown } | null }));
vi.mock("@/store", () => ({
  useAppStore: {
    getState: () => held.store?.getState(),
  },
}));

import { createAutopilotSlice } from "./autopilot";
import { createAutopilotLogSlice } from "./autopilotLog";
import { autopilotPass } from "./autopilotSync";
import { checkoutKey, createGitSlice } from "./git";
import type { AppState } from "./types";

const git = (over: Partial<GitState> = {}): GitState => ({
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
});
const pr = (over: Partial<PrState> = {}): PrState => ({
  number: 7,
  url: "https://x",
  state: "open",
  title: "t",
  mergeable: "mergeable",
  ...over,
});
const checks = (over: Partial<PrChecks> = {}): PrChecks => ({
  merge_state: "blocked",
  rollup: "failing",
  total: 1,
  passed: 0,
  failed: 1,
  pending: 0,
  required_failing: ["test"],
  runs: [],
  ...over,
});
const passing: VerificationReport = {
  checks: [{ name: "test", command: "run test", outcome: "passed", duration_ms: 1, tail: [] }],
};

const state = (over: Partial<AutopilotState> = {}): AutopilotState => ({
  ...newEnrollment(),
  ...over,
});
const cycle = (over: Partial<Cycle> = {}): Cycle => ({
  rung: "fix-checks",
  attempt: 1,
  signature: "sha1|test||",
  phase: "working",
  phaseSince: 0,
  ...over,
});

/** The trigger a `fix-checks` dispatch sends, as `appActionMessage` builds it. */
const TRIGGER = '[app-action] fix-checks failing="test"';

interface Fixture {
  /** Enrolled checkouts, by `checkoutKey`. Their readiness is seeded to the one
   *  world autopilot acts on: an open PR whose required check is failing. */
  autopilot: Record<string, AutopilotState>;
  /** Live agents and their status. A key absent here is an agent that's gone. */
  agents: Record<string, AgentStatus>;
}

function makeStore({ autopilot, agents }: Fixture) {
  const sendUserMessage = vi.fn();
  const store = create<AppState>()(
    (...a) =>
      ({
        ...createGitSlice(...a),
        ...createAutopilotSlice(...a),
        // The applier records every meaningful effect to the audit log, so the
        // store it runs against needs that slice too.
        ...createAutopilotLogSlice(...a),
      }) as AppState,
  );
  const per = <T>(value: T) => Object.fromEntries(Object.keys(autopilot).map((k) => [k, value]));
  store.setState({
    sendUserMessage,
    workspace: {
      agents: Object.entries(agents).map(([id, status]) => ({ id, status })),
      // biome-ignore lint/suspicious/noExplicitAny: minimal workspace fixture
    } as any,
    autopilot,
    gitStates: per(git()),
    prStates: per(pr()),
    prChecks: per(checks()),
    prComments: per({ unresolved: [] }),
    // biome-ignore lint/suspicious/noExplicitAny: partial store seed
  } as any);
  held.store = store;
  return { store, sendUserMessage };
}

const key = { primary: checkoutKey("a1"), web: checkoutKey("a1", "web") };

beforeEach(() => {
  runVerification.mockReset();
  runVerification.mockResolvedValue(passing);
});

describe("autopilotPass dispatches at most once per agent per pass", () => {
  it("hands a rung to only ONE checkout of a multi-repo agent", async () => {
    // Both checkouts are enrolled, both blocked on failing checks, and the agent
    // reads `idle` for both because the status flip from the first dispatch
    // hasn't come back yet. Dispatching both would coalesce the triggers.
    const { store, sendUserMessage } = makeStore({
      autopilot: { [key.primary]: state(), [key.web]: state() },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary, key.web], new Set());

    expect(Object.keys(store.getState().delegations)).toEqual([key.primary]);
    expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith("a1", TRIGGER);
    // The loser is left completely untouched — no cycle was opened for it, so
    // the next tick re-derives it against a world that now shows a busy agent.
    expect(store.getState().autopilot[key.primary].cycle).not.toBeNull();
    expect(store.getState().autopilot[key.web].cycle).toBeNull();
  });

  it("still dispatches concurrently for different agents", async () => {
    // The guard is per agent, not global: two agents can each take a turn.
    const { store, sendUserMessage } = makeStore({
      autopilot: { a1: state(), a2: state() },
      agents: { a1: "idle", a2: "idle" },
    });

    await autopilotPass(["a1", "a2"], new Set());

    expect(Object.keys(store.getState().delegations).sort()).toEqual(["a1", "a2"]);
    expect(sendUserMessage).toHaveBeenCalledTimes(2);
  });

  it("lets the loser dispatch on the NEXT pass, once its sibling turn is done", async () => {
    // The set is per pass, so nothing is remembered: the deferral costs one tick,
    // it doesn't blacklist the checkout.
    const { store, sendUserMessage } = makeStore({
      autopilot: { [key.primary]: state(), [key.web]: state() },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary, key.web], new Set());
    // That turn landed: the primary's checks are green, its cycle settled and its
    // delegation was cleared — so this pass has nothing for the primary, and the
    // sibling deferred a tick ago takes its turn.
    store.getState().clearDelegation(key.primary);
    store.setState({
      autopilot: { ...store.getState().autopilot, [key.primary]: state() },
      prChecks: {
        ...store.getState().prChecks,
        [key.primary]: checks({
          merge_state: "clean",
          rollup: "passing",
          passed: 1,
          failed: 0,
          required_failing: [],
        }),
      },
    });
    await autopilotPass([key.primary, key.web], new Set());

    expect(store.getState().autopilot[key.web].cycle).not.toBeNull();
    expect(sendUserMessage).toHaveBeenLastCalledWith(
      "a1",
      '[app-action] fix-checks failing="test" repo="web"',
    );
  });
});

describe("autopilotPass applies a dispatch", () => {
  it("opens the cycle AND sends the delegation, so the turn is tracked", async () => {
    const { store, sendUserMessage } = makeStore({
      autopilot: { [key.primary]: state() },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary], new Set());

    // The cycle records the rung and the world it started from, which is what
    // makes a barren retry detectable.
    expect(store.getState().autopilot[key.primary].cycle).toEqual({
      rung: "fix-checks",
      attempt: 1,
      signature: "sha1|test||",
      phase: "working",
      phaseSince: 0,
    });
    // Without the delegation nothing would ever advance the cycle out of
    // `working` — the driver depends on the delegation layer's lifecycle.
    expect(store.getState().delegations[key.primary].kind).toBe("fix-checks");
    expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith("a1", TRIGGER);
  });

  it("scopes a secondary checkout's trigger to its repo", async () => {
    // Without `repo=`, the agent would run the fix in the primary repo — the
    // wrong checkout entirely.
    const { store, sendUserMessage } = makeStore({
      autopilot: { [key.web]: state() },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.web], new Set());

    expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith(
      "a1",
      '[app-action] fix-checks failing="test" repo="web"',
    );
    expect(store.getState().delegations[key.web].subdir).toBe("web");
  });
});

describe("autopilotPass applies a verification", () => {
  /** A cycle mid-turn on a `fix-checks` rung, with the agent now settled — the
   *  one shape that produces a `verify` effect. */
  const working: Fixture = {
    autopilot: { [key.primary]: state({ cycle: cycle() }) },
    agents: { a1: "idle" },
  };

  it("enters awaiting-evidence and records the verdict for this cycle", async () => {
    const { store } = makeStore(working);

    await autopilotPass([key.primary], new Set());

    const advanced = store.getState().autopilot[key.primary].cycle;
    expect(advanced?.phase).toBe("awaiting-evidence");
    // The phase clock has to start, or the evidence timeout never fires.
    expect(advanced?.phaseSince).toBeGreaterThan(0);
    expect(runVerification).toHaveBeenCalledExactlyOnceWith("a1", undefined);
    expect(store.getState().autopilotVerdicts[key.primary]).toEqual(passing);
  });

  it("does not let a verification that COULDN'T run look like a fix that didn't work", async () => {
    // A crashed verifier says nothing about the code. The phase must still
    // advance (so the cycle is judged on CI, bounded by the evidence timeout)
    // and no verdict may be recorded, because a recorded failure would burn a
    // budget slot on a cycle that was possibly fine.
    runVerification.mockRejectedValue(new Error("no verify command"));
    const { store } = makeStore(working);
    const verifying = new Set<string>();

    await autopilotPass([key.primary], verifying);

    expect(store.getState().autopilot[key.primary].cycle?.phase).toBe("awaiting-evidence");
    expect(store.getState().autopilotVerdicts).toEqual({});
    // And the in-flight marker is released, or the checkout could never be
    // verified again.
    expect(verifying.size).toBe(0);
  });

  it("issues only one verification per checkout while one is in flight", async () => {
    // Two overlapping passes: `usePoll` guards re-entry per mounted effect, but a
    // dependency change starts a fresh effect while a slow verify is still out.
    let release!: (r: VerificationReport) => void;
    runVerification.mockReturnValueOnce(
      new Promise<VerificationReport>((r) => {
        release = r;
      }),
    );
    const { store } = makeStore(working);
    const verifying = new Set<string>();

    const first = autopilotPass([key.primary], verifying);
    await autopilotPass([key.primary], verifying);

    expect(runVerification).toHaveBeenCalledTimes(1);
    release(passing);
    await first;
    expect(store.getState().autopilotVerdicts[key.primary]).toEqual(passing);
  });
});

describe("autopilotPass drops an enrollment whose agent is gone", () => {
  it("unenrolls rather than ticking forever against nothing", async () => {
    // Archiving or discarding an agent leaves its persisted enrollment behind.
    const { store, sendUserMessage } = makeStore({
      autopilot: { [key.primary]: state(), [key.web]: state() },
      agents: {},
    });

    await autopilotPass([key.primary, key.web], new Set());

    expect(store.getState().autopilot).toEqual({});
    expect(sendUserMessage).not.toHaveBeenCalled();
  });
});

// ── audit log wiring ─────────────────────────────────────────────────────────
// The log slice is tested in isolation (autopilotLog.test.ts); what is only
// testable here is that the applier actually FEEDS it, with the attempt number
// read at the point it is correct. A dispatch has no cycle until
// `openAutopilotCycle` creates one, so logging too early records `undefined`.

describe("autopilotPass records what it did", () => {
  it("logs a dispatch with the attempt number of the cycle it just opened", async () => {
    const { store } = makeStore({
      autopilot: { [key.primary]: state() },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary], new Set());

    const log = store.getState().autopilotLog[key.primary];
    expect(log).toHaveLength(1);
    expect(log[0]).toMatchObject({ outcome: "dispatch", rung: "fix-checks", attempt: 1 });
    expect(log[0].at).toBeGreaterThan(0);
  });

  it("carries the attempt number forward, so a retry reads as the try it was", async () => {
    // Second cycle on the same rung: the entry has to say #2, which is the whole
    // reason the attempt is read from the cycle rather than counted in the log.
    const { store } = makeStore({
      autopilot: { [key.primary]: state({ attempts: { "fix-checks": 1 } }) },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary], new Set());

    expect(store.getState().autopilotLog[key.primary][0]).toMatchObject({
      outcome: "dispatch",
      attempt: 2,
    });
  });

  it("logs an escalation with the reason that stopped it", async () => {
    // Budget spent: the pass escalates instead of dispatching, and the log has to
    // record WHY — that reason is the only explanation the user ever gets.
    const { store } = makeStore({
      autopilot: { [key.primary]: state({ attempts: { "fix-checks": 3 } }) },
      agents: { a1: "idle" },
    });

    await autopilotPass([key.primary], new Set());

    expect(store.getState().autopilotLog[key.primary][0]).toMatchObject({
      outcome: "escalate",
      reason: "budget-spent",
      rung: "fix-checks",
    });
  });

  it("stays silent on a tick with nothing to do", async () => {
    // `wait` is most ticks. Logging them would bury the handful of entries that
    // explain what actually happened.
    const { store } = makeStore({
      autopilot: { [key.primary]: state() },
      agents: { a1: "running" },
    });

    await autopilotPass([key.primary], new Set());

    expect(store.getState().autopilotLog[key.primary]).toBeUndefined();
  });
});
