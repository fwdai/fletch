// The slice behind autopilot: what persists, what deliberately doesn't, and the
// transitions the driver applies. The pure policy is tested in autopilot.test.ts
// at the root; this covers the state it reads.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

// `vi.mock` is hoisted above the module body, so the spy has to be too.
const { setSetting } = vi.hoisted(() => ({ setSetting: vi.fn() }));
vi.mock("@/storage/settings", () => ({ setSetting }));

import { AUTOPILOT_SETTING, createAutopilotSlice, parseAutopilotEnrollment } from "./autopilot";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createAutopilotSlice(...a) }) as AppState);
const report = (outcome: "passed" | "failed") => ({
  checks: [{ name: "test", command: "t", outcome, duration_ms: 1, tail: [] }],
});

describe("enrollment", () => {
  it("starts absent — nothing is enrolled by default", () => {
    expect(makeStore().getState().autopilot).toEqual({});
  });

  it("enrolls clean, with no spent budget and nothing in flight", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    expect(store.getState().autopilot.a1).toEqual({
      enrolled: true,
      paused: false,
      cycle: null,
      attempts: {},
      barren: [],
      stuck: null,
    });
  });

  it("persists only the user's intent, never in-flight machinery", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().openAutopilotCycle("a1", "fix-checks", "sig");
    store.getState().pauseAutopilot("a1");

    // The last write is what a reload would read: enrolled + paused, no cycle.
    const [key, value] = setSetting.mock.calls.at(-1) ?? [];
    expect(key).toBe(AUTOPILOT_SETTING);
    expect(value).toEqual({ a1: { paused: true } });
  });

  it("pausing drops the in-flight cycle but keeps enrollment", () => {
    // The agent's turn isn't interrupted, but autopilot stops judging it — so
    // resuming re-derives from the world instead of scoring a turn nobody watched.
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().openAutopilotCycle("a1", "fix-checks", "sig");
    store.getState().pauseAutopilot("a1");

    expect(store.getState().autopilot.a1.cycle).toBeNull();
    expect(store.getState().autopilot.a1.enrolled).toBe(true);
  });

  it("resuming is the only thing that clears stuck, and it clears the budget with it", () => {
    // `stuck` is sticky by design: a human got it there, a human gets it out. The
    // spent attempts and barren signatures go too — otherwise "try again" would
    // immediately give up again.
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().retryAutopilotCycle("a1", "fix-checks", "sig");
    store
      .getState()
      .markAutopilotStuck("a1", "budget-spent", "fix-checks", 5, "checks-failing:test");
    expect(store.getState().autopilot.a1.stuck).not.toBeNull();

    store.getState().resumeAutopilot("a1");
    const s = store.getState().autopilot.a1;
    expect(s.stuck).toBeNull();
    expect(s.attempts).toEqual({});
    expect(s.barren).toEqual([]);
  });

  it("unenrolling forgets the checkout entirely", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().enrollAutopilot("a1::web");
    store.getState().unenrollAutopilot("a1");

    expect(Object.keys(store.getState().autopilot)).toEqual(["a1::web"]);
    expect(setSetting.mock.calls.at(-1)?.[1]).toEqual({ "a1::web": { paused: false } });
  });

  it("ignores transitions for a checkout that was never enrolled", () => {
    // The driver only ticks enrolled keys, but a race (unenroll mid-tick) must
    // not resurrect an entry.
    const store = makeStore();
    store.getState().openAutopilotCycle("ghost", "fix-checks", "sig");
    store.getState().markAutopilotStuck("ghost", "no-progress", null, 1, "");
    expect(store.getState().autopilot.ghost).toBeUndefined();
  });
});

describe("cycle bookkeeping", () => {
  it("numbers attempts from the rung's spent budget", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().openAutopilotCycle("a1", "fix-checks", "sig");
    expect(store.getState().autopilot.a1.cycle?.attempt).toBe(1);

    store.getState().retryAutopilotCycle("a1", "fix-checks", null);
    store.getState().openAutopilotCycle("a1", "fix-checks", "sig2");
    expect(store.getState().autopilot.a1.cycle?.attempt).toBe(2);
  });

  it("records a barren signature once, and only when given one", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().retryAutopilotCycle("a1", "fix-checks", "sig");
    store.getState().retryAutopilotCycle("a1", "fix-checks", "sig");
    store.getState().retryAutopilotCycle("a1", "fix-checks", null);
    expect(store.getState().autopilot.a1.barren).toEqual(["sig"]);
    expect(store.getState().autopilot.a1.attempts).toEqual({ "fix-checks": 3 });
  });

  it("gives the budget back on success, so a long-lived PR isn't capped for life", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().retryAutopilotCycle("a1", "fix-checks", null);
    store.getState().retryAutopilotCycle("a1", "fix-checks", null);
    store.getState().settleAutopilotCycle("a1", "fix-checks");
    expect(store.getState().autopilot.a1.attempts["fix-checks"]).toBe(0);
    expect(store.getState().autopilot.a1.cycle).toBeNull();
  });

  it("stamps the phase clock when evidence starts being awaited", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().openAutopilotCycle("a1", "fix-checks", "sig");
    store.getState().advanceAutopilotCycle("a1", "awaiting-evidence", 4242);
    expect(store.getState().autopilot.a1.cycle).toMatchObject({
      phase: "awaiting-evidence",
      phaseSince: 4242,
    });
  });
});

describe("verdicts belong to the cycle that produced them", () => {
  it("drops the previous cycle's verdict when a new cycle opens", () => {
    // Otherwise a stale "tests failed" from the last attempt would immediately
    // condemn the next one.
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().recordAutopilotVerdict("a1", report("failed"));
    expect(store.getState().autopilotVerdicts.a1).toBeDefined();

    store.getState().openAutopilotCycle("a1", "fix-checks", "sig");
    expect(store.getState().autopilotVerdicts.a1).toBeUndefined();
  });

  it("keys verdicts per checkout, so a secondary repo gets its own evidence", () => {
    // The existing `verificationReports` map is agent-keyed, which is why
    // autopilot keeps its own: a secondary checkout would otherwise overwrite the
    // primary's report and be judged by it.
    const store = makeStore();
    store.getState().recordAutopilotVerdict("a1", report("passed"));
    store.getState().recordAutopilotVerdict("a1::web", report("failed"));
    expect(store.getState().autopilotVerdicts.a1.checks[0].outcome).toBe("passed");
    expect(store.getState().autopilotVerdicts["a1::web"].checks[0].outcome).toBe("failed");
  });
});

describe("parseAutopilotEnrollment", () => {
  it("restores enrollment and the paused flag", () => {
    const parsed = parseAutopilotEnrollment(JSON.stringify({ a1: { paused: true }, a2: {} }));
    expect(parsed.a1).toMatchObject({ enrolled: true, paused: true });
    expect(parsed.a2).toMatchObject({ enrolled: true, paused: false });
  });

  it("never restores a cycle, a spent budget, or a stuck the user didn't see", () => {
    // A hand-edited or corrupt row must not be able to inject machinery — the
    // persisted shape carries intent only.
    const parsed = parseAutopilotEnrollment(
      JSON.stringify({
        a1: { paused: false, cycle: { rung: "fix-checks" }, attempts: { "fix-checks": 3 } },
      }),
    );
    expect(parsed.a1).toEqual({
      enrolled: true,
      paused: false,
      cycle: null,
      attempts: {},
      barren: [],
      stuck: null,
    });
  });

  it("fails closed on junk or absence — nothing enrolled", () => {
    // The wrong way to fail is "enroll everything"; a loop that spends agent
    // turns must default to off.
    expect(parseAutopilotEnrollment(undefined)).toEqual({});
    expect(parseAutopilotEnrollment("")).toEqual({});
    expect(parseAutopilotEnrollment("{oops")).toEqual({});
  });
});
