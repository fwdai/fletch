import { describe, expect, it } from "vitest";
import type { AutopilotState, CyclePhase, StuckReason } from "@/autopilot";
import { newEnrollment } from "@/autopilot";
import { autopilotSignal, autopilotTip } from "./autopilotSignal";

const state = (over: Partial<AutopilotState> = {}): AutopilotState => ({
  ...newEnrollment(),
  ...over,
});

const working = (attempt = 1, phase: CyclePhase = "working") =>
  state({ cycle: { rung: "fix-checks", attempt, signature: "s", phase, phaseSince: 0 } });

const stuck = (reason: StuckReason) =>
  state({ stuck: { reason, rung: "fix-checks", at: 1, blockers: "" } });

describe("autopilotSignal", () => {
  it("is absent when the agent has no enrolled checkout", () => {
    expect(autopilotSignal({}, "a")).toBeNull();
    expect(autopilotSignal({ a: state({ enrolled: false }) }, "a")).toBeNull();
  });

  it("is absent for an enrolled-but-quiet checkout — on is the norm, not news", () => {
    expect(autopilotSignal({ a: state() }, "a")).toBeNull();
  });

  it("is absent while waiting for CI — the PR pill already says checks are running", () => {
    expect(autopilotSignal({ a: working(1, "awaiting-evidence") }, "a")).toBeNull();
  });

  it("ignores other agents' checkouts, including id prefixes", () => {
    const map = { b: working(), ab: working(), "ab::web": working() };
    expect(autopilotSignal(map, "a")).toBeNull();
  });

  it("sees a secondary checkout's state and names the repo", () => {
    expect(autopilotSignal({ "a::web": working() }, "a")).toMatchObject({
      mode: "working",
      repo: "web",
      rung: "fix-checks",
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

  it("keeps the first checkout on a tie, so the row doesn't flip between equals", () => {
    expect(autopilotSignal({ "a::api": working(), "a::web": working() }, "a")).toMatchObject({
      repo: "api",
    });
    expect(
      autopilotSignal({ "a::api": stuck("no-progress"), "a::web": stuck("needs-human") }, "a"),
    ).toMatchObject({ repo: "api", reason: "no-progress" });
  });

  it("carries the in-flight attempt count", () => {
    expect(autopilotSignal({ a: working(3) }, "a")).toMatchObject({ mode: "working", attempt: 3 });
  });
});

describe("autopilotTip", () => {
  it("explains a stuck checkout as a fact about the PR, never naming autopilot", () => {
    const tip = autopilotTip({
      mode: "stuck",
      repo: null,
      rung: "fix-checks",
      attempt: null,
      reason: "budget-spent",
    });
    expect(tip).toBe("Gave up on the failing checks after 3 tries");
    expect(tip).not.toMatch(/autopilot/i);
  });

  it("names the repo so a multi-repo agent's mark isn't ambiguous", () => {
    expect(
      autopilotTip({ mode: "working", repo: "web", rung: "resolve", attempt: 1, reason: null }),
    ).toBe("Working on the conflicts automatically (web)");
  });

  it("mentions the attempt only once retrying means something", () => {
    const tip = (attempt: number) =>
      autopilotTip({ mode: "working", repo: null, rung: "fix-checks", attempt, reason: null });
    expect(tip(1)).toBe("Working on the failing checks automatically");
    expect(tip(2)).toBe("Working on the failing checks automatically, try 2");
  });
});
