import { describe, expect, it } from "vitest";
import type { AutopilotState, CyclePhase } from "@/autopilot";
import { newEnrollment } from "@/autopilot";
import { autopilotSignal, autopilotTip } from "./autopilotSignal";

const state = (over: Partial<AutopilotState> = {}): AutopilotState => ({
  ...newEnrollment(),
  ...over,
});

const working = (attempt = 1, phase: CyclePhase = "working") =>
  state({ cycle: { rung: "fix-checks", attempt, signature: "s", phase, phaseSince: 0 } });

describe("autopilotSignal", () => {
  it("is absent when the agent has no enrolled checkout", () => {
    expect(autopilotSignal({}, "a")).toBeNull();
    expect(autopilotSignal({ a: state({ enrolled: false }) }, "a")).toBeNull();
  });

  it("is absent for an enrolled-but-quiet checkout — on is the norm, not news", () => {
    expect(autopilotSignal({ a: state() }, "a")).toBeNull();
  });

  it("is absent for a checkout autopilot gave up on — that is just a PR, not a mark", () => {
    // A spent budget and a barren world are autopilot's own bookkeeping. The row
    // says nothing; the Git panel's history says what happened, if asked.
    const spent = state({
      attempts: { "fix-checks": 3 },
      situation: "checks-failing:test",
      barren: ["sha1|test||"],
    });
    expect(autopilotSignal({ a: spent }, "a")).toBeNull();
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
  it("names the repo so a multi-repo agent's mark isn't ambiguous", () => {
    expect(autopilotTip({ mode: "working", repo: "web", rung: "resolve", attempt: 1 })).toBe(
      "Working on the conflicts automatically (web)",
    );
  });

  it("mentions the attempt only once retrying means something", () => {
    const tip = (attempt: number) =>
      autopilotTip({ mode: "working", repo: null, rung: "fix-checks", attempt });
    expect(tip(1)).toBe("Working on the failing checks automatically");
    expect(tip(2)).toBe("Working on the failing checks automatically, try 2");
  });

  it("never names autopilot — the mark is a fact about the PR", () => {
    expect(
      autopilotTip({ mode: "working", repo: null, rung: "fix-checks", attempt: 1 }),
    ).not.toMatch(/autopilot/i);
  });
});
