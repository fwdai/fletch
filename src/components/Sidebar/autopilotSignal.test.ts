import { describe, expect, it } from "vitest";
import type { AutopilotState, StuckReason } from "@/autopilot";
import { autopilotSignal, autopilotTip } from "./autopilotSignal";

function state(over: Partial<AutopilotState> = {}): AutopilotState {
  return {
    enrolled: true,
    paused: false,
    cycle: null,
    attempts: {},
    barren: [],
    stuck: null,
    ...over,
  };
}

const working = (attempt = 1) =>
  state({
    cycle: { rung: "fix-checks", attempt, signature: "s", phase: "working", phaseSince: 0 },
  });

const stuck = (reason: StuckReason) =>
  state({ stuck: { reason, rung: "fix-checks", at: 1, blockers: "" } });

describe("autopilotSignal", () => {
  it("is absent when the agent has no enrolled checkout", () => {
    expect(autopilotSignal({}, "a")).toBeNull();
    expect(autopilotSignal({ a: state({ enrolled: false }) }, "a")).toBeNull();
  });

  it("reports an enrolled-but-quiet checkout as idle, not as nothing", () => {
    expect(autopilotSignal({ a: state() }, "a")).toMatchObject({ mode: "idle", repo: null });
  });

  it("ignores other agents' checkouts, including id prefixes", () => {
    const map = { b: working(), ab: working(), "ab::web": working() };
    expect(autopilotSignal(map, "a")).toBeNull();
  });

  it("sees a secondary checkout's state and names the repo", () => {
    expect(autopilotSignal({ "a::web": working() }, "a")).toMatchObject({
      mode: "working",
      repo: "web",
    });
  });

  it("keeps the first `::` so a subdir containing the separator round-trips", () => {
    expect(autopilotSignal({ "a::web::v2": working() }, "a")).toMatchObject({ repo: "web::v2" });
  });

  it("prefers stuck over working, whichever checkout it came from", () => {
    expect(autopilotSignal({ a: working(), "a::web": stuck("budget-spent") }, "a")).toMatchObject({
      mode: "stuck",
      repo: "web",
      reason: "budget-spent",
    });
    // ...and the other way round, so the answer can't be an artifact of key order.
    expect(autopilotSignal({ "a::web": stuck("budget-spent"), a: working() }, "a")).toMatchObject({
      mode: "stuck",
      repo: "web",
    });
  });

  it("prefers working over paused and idle", () => {
    expect(
      autopilotSignal({ a: state(), "a::api": state({ paused: true }), "a::web": working() }, "a"),
    ).toMatchObject({ mode: "working", repo: "web" });
  });

  it("prefers paused over idle", () => {
    expect(autopilotSignal({ a: state(), "a::web": state({ paused: true }) }, "a")).toMatchObject({
      mode: "paused",
      repo: "web",
    });
  });

  it("keeps the first checkout on a tie, so the row doesn't flip between equals", () => {
    expect(autopilotSignal({ "a::api": working(), "a::web": working() }, "a")).toMatchObject({
      repo: "api",
    });
  });

  it("carries the in-flight attempt count", () => {
    expect(autopilotSignal({ a: working(3) }, "a")).toMatchObject({ mode: "working", attempt: 3 });
  });
});

describe("autopilotTip", () => {
  it("explains a stuck checkout with the same words as the git-panel chip", () => {
    const tip = autopilotTip({
      mode: "stuck",
      repo: null,
      attempt: null,
      reason: "needs-human",
    });
    expect(tip).toBe("Autopilot stopped — this needs you");
  });

  it("names the repo so a multi-repo agent's mark isn't ambiguous", () => {
    expect(autopilotTip({ mode: "working", repo: "web", attempt: 1, reason: null })).toBe(
      "Autopilot working (web)",
    );
  });

  it("mentions the attempt only once retrying means something", () => {
    expect(autopilotTip({ mode: "working", repo: null, attempt: 1, reason: null })).toBe(
      "Autopilot working",
    );
    expect(autopilotTip({ mode: "working", repo: null, attempt: 2, reason: null })).toBe(
      "Autopilot working, attempt 2",
    );
  });
});
