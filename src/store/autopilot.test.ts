// The slice behind autopilot: what persists, what deliberately doesn't, and the
// transitions the driver applies. The pure policy is tested in autopilot.test.ts
// at the root; this covers the state it reads.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { create } from "zustand";

// `vi.mock` is hoisted above the module body, so the spies have to be too.
const { setSetting, setProjectSetting, deleteProjectSetting, loadAutopilotDisabledProjects } =
  vi.hoisted(() => ({
    setSetting: vi.fn(),
    setProjectSetting: vi.fn(() => Promise.resolve()),
    deleteProjectSetting: vi.fn(() => Promise.resolve()),
    loadAutopilotDisabledProjects: vi.fn(() => Promise.resolve([] as string[])),
  }));
vi.mock("@/storage/settings", () => ({ setSetting }));
vi.mock("@/storage/projectSettings", () => ({
  AUTOPILOT_ENABLED_KEY: "autopilot.enabled",
  setProjectSetting,
  deleteProjectSetting,
  loadAutopilotDisabledProjects,
}));

import {
  AUTOPILOT_SETTING,
  autopilotProjectOn,
  createAutopilotSlice,
  parseAutopilotEnrollment,
} from "./autopilot";
import type { AppState } from "./types";

const makeStore = () =>
  create<AppState>()((...a) => ({ ...createAutopilotSlice(...a) }) as AppState);
const report = (outcome: "passed" | "failed") => ({
  checks: [{ name: "test", command: "t", outcome, duration_ms: 1, tail: [] }],
});

/** An async op the test releases by hand, to order completions deliberately. */
const deferred = <T = void>() => {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

// Every test that writes uses its own project id: the per-project write queue
// and sequence live at module scope, like the writes they order, so a queued
// write from one test must not be able to trail into the next one's expectations.
let n = 0;
const fresh = () => `p${++n}`;

beforeEach(() => {
  setProjectSetting.mockReset().mockImplementation(() => Promise.resolve());
  deleteProjectSetting.mockReset().mockImplementation(() => Promise.resolve());
  loadAutopilotDisabledProjects.mockReset().mockImplementation(() => Promise.resolve([]));
  vi.spyOn(console, "error").mockImplementation(() => {});
});

describe("autopilotProjectOn", () => {
  it("is on for every project not listed, and off for a listed one", () => {
    expect(autopilotProjectOn([], "p1")).toBe(true);
    expect(autopilotProjectOn(["p1"], "p1")).toBe(false);
    expect(autopilotProjectOn(["p1"], "p2")).toBe(true);
  });

  it("is off for EVERY project while the opt-outs are unknown", () => {
    // The one predicate the driver, the chip and the toggle all share, so
    // "unknown" fails closed everywhere at once rather than in three places.
    expect(autopilotProjectOn(null, "p1")).toBe(false);
  });
});

describe("project switch: loading", () => {
  it("starts with the opt-outs unknown (null), not with everything on", () => {
    // Until hydration fills the list, the driver must run nothing: on-by-default
    // without knowing who opted out would act on exactly the wrong projects.
    expect(makeStore().getState().autopilotDisabledProjects).toBeNull();
  });

  it("loads the list, and a failed load leaves it unknown rather than empty", async () => {
    const store = makeStore();
    loadAutopilotDisabledProjects.mockRejectedValueOnce(new Error("db not ready"));
    await store.getState().loadAutopilotProjects();
    expect(store.getState().autopilotDisabledProjects).toBeNull();

    // The same action is the retry: the settings section calls it again.
    loadAutopilotDisabledProjects.mockResolvedValueOnce(["p9"]);
    await store.getState().loadAutopilotProjects();
    expect(store.getState().autopilotDisabledProjects).toEqual(["p9"]);
  });

  it("ignores a load that a later load overtook", async () => {
    // Startup load is slow; the user hits Retry, which finishes first. The
    // startup load then completes with an older snapshot and must not replace
    // the fresher one — or a project the retry saw as off would flip back on.
    const store = makeStore();
    const slow = deferred<string[]>();
    loadAutopilotDisabledProjects.mockReturnValueOnce(slow.promise);
    const first = store.getState().loadAutopilotProjects();

    loadAutopilotDisabledProjects.mockResolvedValueOnce(["p-off"]);
    await store.getState().loadAutopilotProjects();
    expect(store.getState().autopilotDisabledProjects).toEqual(["p-off"]);

    slow.resolve([]);
    await first;
    expect(store.getState().autopilotDisabledProjects).toEqual(["p-off"]);
  });

  it("ignores a load that a click overtook, in the store AND in the known row values", async () => {
    // Loaded, then a load is in flight (Retry) when the user opts a project out
    // and the write succeeds. The load's snapshot predates that opt-out: landing
    // it would show the project on, run autopilot on it, and make a later failed
    // write roll back to "on" — the wrong row value.
    const store = makeStore();
    const p = fresh();
    loadAutopilotDisabledProjects.mockResolvedValueOnce([]);
    await store.getState().loadAutopilotProjects();

    const slow = deferred<string[]>();
    loadAutopilotDisabledProjects.mockReturnValueOnce(slow.promise);
    const stale = store.getState().loadAutopilotProjects();

    store.getState().setProjectAutopilot(p, false);
    await vi.waitFor(() => expect(setProjectSetting).toHaveBeenCalledTimes(1));
    slow.resolve([]); // snapshot from before the opt-out
    await stale;
    expect(store.getState().autopilotDisabledProjects).toEqual([p]);

    // The known row value survived too: a failed "on" now reverts to off.
    deleteProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    store.getState().setProjectAutopilot(p, true);
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([p]));
  });

  it("refuses to flip a switch while the opt-outs are unknown", () => {
    // Applying a click on top of null would invent an empty list and switch
    // every project on — the exact failure null exists to prevent. The store
    // enforces it, so no caller (not just the disabled toggle) can do it.
    const store = makeStore();
    store.getState().setProjectAutopilot(fresh(), true);
    expect(store.getState().autopilotDisabledProjects).toBeNull();
    expect(deleteProjectSetting).not.toHaveBeenCalled();
    expect(setProjectSetting).not.toHaveBeenCalled();
  });
});

describe("project switch: writing", () => {
  const loaded = () => {
    const store = makeStore();
    store.setState({ autopilotDisabledProjects: [] });
    return store;
  };

  it("turning a project off writes the one row that exists; turning it on deletes it", async () => {
    // On is the default, so "on" is the ABSENCE of a row — a project never
    // touched and a project switched back on look identical in the table.
    const store = loaded();
    const p = fresh();
    store.getState().setProjectAutopilot(p, false);
    expect(store.getState().autopilotDisabledProjects).toEqual([p]);
    await vi.waitFor(() =>
      expect(setProjectSetting).toHaveBeenLastCalledWith(p, "autopilot.enabled", "0"),
    );

    store.getState().setProjectAutopilot(p, true);
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
    await vi.waitFor(() =>
      expect(deleteProjectSetting).toHaveBeenLastCalledWith(p, "autopilot.enabled"),
    );
  });

  it("switching a project off twice records it once", () => {
    const store = loaded();
    const p = fresh();
    store.getState().setProjectAutopilot(p, false);
    store.getState().setProjectAutopilot(p, false);
    expect(store.getState().autopilotDisabledProjects).toEqual([p]);
  });

  it("reverts the switch when the durable write fails", async () => {
    // The row is the truth: a session that shows "off" while the table still
    // says "on" would quietly resume autopilot on the next launch. Better to
    // snap the toggle back so the user sees the change didn't take.
    setProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    const store = loaded();
    const p = fresh();

    store.getState().setProjectAutopilot(p, false);
    expect(store.getState().autopilotDisabledProjects).toEqual([p]);
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([]));
  });

  it("runs one project's writes in click order, even when the first is slow", async () => {
    // off (slow) then on (fast): without ordering the delete lands first and the
    // slow upsert then persists "off" — the opposite of the last click.
    const slow = deferred();
    setProjectSetting.mockReturnValueOnce(slow.promise);
    const store = loaded();
    const p = fresh();

    store.getState().setProjectAutopilot(p, false);
    store.getState().setProjectAutopilot(p, true);
    await vi.waitFor(() => expect(setProjectSetting).toHaveBeenCalledTimes(1));
    expect(deleteProjectSetting).not.toHaveBeenCalled();

    slow.resolve();
    await vi.waitFor(() => expect(deleteProjectSetting).toHaveBeenCalledTimes(1));
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
  });

  it("a stale failure does not roll back a later choice", async () => {
    // off fails slowly while on has already been requested: the user's latest
    // choice is "on", and the earlier failure must not be allowed to touch it.
    // Only the latest request for a project may roll back.
    const slow = deferred();
    setProjectSetting.mockReturnValueOnce(slow.promise);
    const store = loaded();
    const p = fresh();

    store.getState().setProjectAutopilot(p, false); // slow, will fail
    store.getState().setProjectAutopilot(p, true); // queued behind it
    expect(store.getState().autopilotDisabledProjects).toEqual([]);

    slow.reject(new Error("db locked"));
    await vi.waitFor(() => expect(deleteProjectSetting).toHaveBeenCalledTimes(1));
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
  });

  it("a failure of the LATEST request does roll back, even behind an earlier success", async () => {
    // Mirror image: the earlier write succeeds, the latest one fails, so the
    // store must return to what the earlier write persisted.
    deleteProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    const store = loaded();
    const p = fresh();

    store.getState().setProjectAutopilot(p, false); // succeeds
    store.getState().setProjectAutopilot(p, true); // fails
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([p]));
  });

  it("rolls back to what the row is KNOWN to hold, not to the click before", async () => {
    // off fails, then on fails. The inverse of the latest click would be "off",
    // but neither write changed the row — it still holds the default (on). A
    // store showing off here would have autopilot resume at the next launch
    // behind a switch that says otherwise.
    setProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    deleteProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    const store = loaded();
    const p = fresh();

    store.getState().setProjectAutopilot(p, false); // fails
    store.getState().setProjectAutopilot(p, true); // fails
    await vi.waitFor(() => expect(deleteProjectSetting).toHaveBeenCalledTimes(1));
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([]));
    // And the mirror: on fails, then off fails, from a row that holds off.
    const q = fresh();
    store.setState({ autopilotDisabledProjects: [q] });
    loadAutopilotDisabledProjects.mockResolvedValueOnce([q]);
    await store.getState().loadAutopilotProjects();
    deleteProjectSetting.mockRejectedValueOnce(new Error("db locked"));
    setProjectSetting.mockRejectedValueOnce(new Error("db locked"));

    store.getState().setProjectAutopilot(q, true); // fails
    store.getState().setProjectAutopilot(q, false); // fails
    await vi.waitFor(() => expect(setProjectSetting).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([q]));
  });

  it("a load seeds what the row holds, so the first failed write reverts to it", async () => {
    // The loaded list is the truth as of launch: a project the table says is off
    // must revert to off when its very first write of the session fails.
    const store = makeStore();
    const p = fresh();
    loadAutopilotDisabledProjects.mockResolvedValueOnce([p]);
    await store.getState().loadAutopilotProjects();
    deleteProjectSetting.mockRejectedValueOnce(new Error("db locked"));

    store.getState().setProjectAutopilot(p, true);
    expect(store.getState().autopilotDisabledProjects).toEqual([]);
    await vi.waitFor(() => expect(store.getState().autopilotDisabledProjects).toEqual([p]));
  });

  it("keeps different projects' writes independent", async () => {
    // Serialization is per project: a slow write for one must not hold up
    // another's.
    const slow = deferred();
    setProjectSetting.mockReturnValueOnce(slow.promise);
    const store = loaded();
    const a = fresh();
    const b = fresh();

    store.getState().setProjectAutopilot(a, false); // slow
    store.getState().setProjectAutopilot(b, false);
    await vi.waitFor(() => expect(setProjectSetting).toHaveBeenCalledTimes(2));
    slow.resolve();
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
