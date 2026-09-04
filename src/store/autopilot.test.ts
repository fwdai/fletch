// The slice behind autopilot: what persists, what deliberately doesn't, and the
// transitions the driver applies. The pure policy is tested in autopilot.test.ts
// at the root; this covers the state it reads.

import { describe, expect, it, vi } from "vitest";
import { create } from "zustand";

// `vi.mock` is hoisted above the module body, so the spies have to be too.
const { setSetting, setProjectSetting, deleteProjectSetting } = vi.hoisted(() => ({
  setSetting: vi.fn(),
  setProjectSetting: vi.fn(() => Promise.resolve()),
  deleteProjectSetting: vi.fn(() => Promise.resolve()),
}));
vi.mock("@/storage/settings", () => ({ setSetting }));
vi.mock("@/storage/projectSettings", () => ({
  AUTOPILOT_ENABLED_KEY: "autopilot.enabled",
  setProjectSetting,
  deleteProjectSetting,
}));

import { AUTOPILOT_SETTING, createAutopilotSlice, parseAutopilotEnrollment } from "./autopilot";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createAutopilotSlice(...a) }) as AppState);
const report = (outcome: "passed" | "failed") => ({
  checks: [{ name: "test", command: "t", outcome, duration_ms: 1, tail: [] }],
});

describe("project switch", () => {
  it("has every project on by default — the disabled list starts empty", () => {
    expect(makeStore().getState().autopilotDisabledProjects).toEqual([]);
  });

  it("turning a project off writes the one row that exists; turning it on deletes it", () => {
    // On is the default, so "on" is the ABSENCE of a row — a project never
    // touched and a project switched back on look identical in the table.
    const store = makeStore();
    store.getState().setProjectAutopilot("p1", false);
    expect(store.getState().autopilotDisabledProjects).toEqual(["p1"]);
    expect(setProjectSetting).toHaveBeenLastCalledWith("p1", "autopilot.enabled", "0");

    store.getState().setProjectAutopilot("p1", true);
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
    expect(deleteProjectSetting).toHaveBeenLastCalledWith("p1", "autopilot.enabled");
  });

  it("switching a project off twice records it once", () => {
    const store = makeStore();
    store.getState().setProjectAutopilot("p1", false);
    store.getState().setProjectAutopilot("p1", false);
    expect(store.getState().autopilotDisabledProjects).toEqual(["p1"]);
  });
});

describe("enrollment", () => {
  it("starts absent — the driver enrolls live checkouts on its first tick", () => {
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

    // The last write is what a reload would read: paused, no cycle.
    const [key, value] = setSetting.mock.calls.at(-1) ?? [];
    expect(key).toBe(AUTOPILOT_SETTING);
    expect(value).toEqual({ a1: { paused: true } });
  });

  it("does not persist a checkout that is merely enrolled", () => {
    // Enrollment is the default and re-derived from live agents every tick, so a
    // row per checkout would only grow; the paused flag is the one intent worth
    // keeping. Resuming therefore removes the entry again.
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    expect(setSetting.mock.calls.at(-1)?.[1]).toEqual({});

    store.getState().pauseAutopilot("a1");
    store.getState().resumeAutopilot("a1");
    expect(setSetting.mock.calls.at(-1)?.[1]).toEqual({});
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

  it("reviving grants a fresh budget but REMEMBERS what it already failed at", () => {
    // Revive is autopilot noticing the world moved, not the user insisting — so
    // unlike `resumeAutopilot` it keeps `barren`. Without that, a checkout whose
    // world oscillates (a flaky check flipping back and forth) would get a full
    // budget on every flip and burn agent turns re-attempting a world it has
    // already proven it cannot change.
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().retryAutopilotCycle("a1", "fix-checks", "dead-world");
    store.getState().markAutopilotStuck("a1", "budget-spent", "fix-checks", 5, "checks-failing:x");

    store.getState().reviveAutopilot("a1");

    const s = store.getState().autopilot.a1;
    expect(s.stuck).toBeNull();
    expect(s.attempts).toEqual({});
    expect(s.barren).toEqual(["dead-world"]);
    // A human insisting DOES clear it — that is the difference between the two.
    store.getState().resumeAutopilot("a1");
    expect(store.getState().autopilot.a1.barren).toEqual([]);
  });

  it("unenrolling forgets the checkout entirely, including its paused flag", () => {
    const store = makeStore();
    store.getState().enrollAutopilot("a1");
    store.getState().enrollAutopilot("a1::web");
    store.getState().pauseAutopilot("a1");
    store.getState().pauseAutopilot("a1::web");
    store.getState().unenrollAutopilot("a1");

    expect(Object.keys(store.getState().autopilot)).toEqual(["a1::web"]);
    expect(setSetting.mock.calls.at(-1)?.[1]).toEqual({ "a1::web": { paused: true } });
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

  it("yields nothing on junk or absence — the driver re-enrolls live checkouts itself", () => {
    // A corrupt row must not be able to pause (or un-pause) anything the user
    // didn't; the only thing lost is the paused set, which the user can redo.
    expect(parseAutopilotEnrollment(undefined)).toEqual({});
    expect(parseAutopilotEnrollment("")).toEqual({});
    expect(parseAutopilotEnrollment("{oops")).toEqual({});
  });
});
