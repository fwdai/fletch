import { describe, expect, it } from "vitest";
import type { AgentRecord, WfRun } from "@/api";
import {
  type ActiveCandidate,
  bubbleActive,
  isLive,
  lastActivity,
} from "@/components/Sidebar/recentProjects";

const DAY = 86_400_000;
const NOW = Date.parse("2026-09-24T12:00:00Z");

function agent(daysAgo: number, status: AgentRecord["status"] = "idle"): AgentRecord {
  return {
    id: `a-${daysAgo}-${status}`,
    project_id: "p",
    name: "a",
    provider: "claude",
    repos: [{ repo_path: "/r", subdir: "" }],
    task: "",
    status,
    view: "custom",
    created_at: new Date(NOW - daysAgo * DAY).toISOString(),
  };
}

function run(daysAgo: number, status: WfRun["status"]): WfRun {
  return { status, created_at: NOW - daysAgo * DAY, updated_at: NOW } as WfRun;
}

function group(key: string, agents: AgentRecord[] = [], runs: WfRun[] = []) {
  return { key, agents, runs };
}

const keys = (list: ActiveCandidate[]) => list.map((g) => g.key);

describe("lastActivity", () => {
  it("is the newest agent or run launch, ignoring what is still running", () => {
    expect(lastActivity(group("p", [agent(3), agent(10, "running")]), {})).toBe(NOW - 3 * DAY);
    expect(lastActivity(group("p", [agent(5)], [run(2, "done")]), {})).toBe(NOW - 2 * DAY);
  });

  it("is the last turn started when that is later than every launch", () => {
    const g = group("p", [agent(3), agent(10)]);
    expect(lastActivity(g, { p: NOW - DAY })).toBe(NOW - DAY);
    expect(lastActivity(g, { p: NOW - 5 * DAY })).toBe(NOW - 3 * DAY);
    expect(lastActivity(g, { other: NOW })).toBe(NOW - 3 * DAY);
  });

  it("is zero for an empty, never-used project", () => {
    expect(lastActivity(group("p"), {})).toBe(0);
  });
});

describe("isLive", () => {
  it("is true while an agent or a run is running", () => {
    expect(isLive(group("p", [agent(30, "running")]))).toBe(true);
    expect(isLive(group("p", [agent(0, "spawning")]))).toBe(true);
    expect(isLive(group("p", [], [run(30, "running")]))).toBe(true);
    expect(isLive(group("p", [agent(0), agent(0, "error")], [run(0, "done")]))).toBe(false);
  });
});

describe("bubbleActive", () => {
  const many = [
    group("a", [agent(30)]),
    group("b", [agent(1)]),
    group("c", [agent(40)]),
    group("d", [agent(0)]),
    group("e", [agent(2)]),
    group("f", [agent(6)]),
    group("g"),
  ];

  it("floats projects launched into within the window, newest first, over the manual order", () => {
    const { active, rest } = bubbleActive(many, NOW);
    expect(keys(active)).toEqual(["d", "b", "e"]);
    expect(keys(rest)).toEqual(["a", "c", "f", "g"]);
  });

  it("ranks a fresh launch above projects whose older agents are still running", () => {
    const list = [
      group("x", [agent(1, "running")]),
      group("y", [agent(0.5, "running")]),
      group("fresh", [agent(0)]),
    ];
    expect(keys(bubbleActive(list, NOW).active)).toEqual(["fresh", "y", "x"]);
  });

  it("moves a project to the top when a message resumes one of its idle agents", () => {
    const list = [group("x", [agent(0.5)]), group("resumed", [agent(20)]), group("y", [agent(1)])];
    expect(keys(bubbleActive(list, NOW).active)).toEqual(["x", "y"]);
    expect(keys(bubbleActive(list, NOW, { resumed: NOW }).active)).toEqual(["resumed", "x", "y"]);
  });

  it("keeps a project in place when its newest agent finishes", () => {
    const before = [group("x", [agent(1, "running")]), group("fresh", [agent(0, "running")])];
    const after = [group("x", [agent(1, "running")]), group("fresh", [agent(0, "idle")])];
    expect(keys(bubbleActive(before, NOW).active)).toEqual(["fresh", "x"]);
    expect(keys(bubbleActive(after, NOW).active)).toEqual(["fresh", "x"]);
  });

  it("keeps a live project floated past the window, ranked by its launch", () => {
    const list = [group("old-live", [agent(20, "running")]), group("b", [agent(1)])];
    expect(keys(bubbleActive(list, NOW).active)).toEqual(["b", "old-live"]);
  });

  it("has no cap: a busy stretch floats every active project", () => {
    const busy = Array.from({ length: 6 }, (_, i) => group(`p${i}`, [agent(i / 4)]));
    expect(bubbleActive([...busy, group("quiet")], NOW).active).toHaveLength(6);
  });

  it("leaves the whole list alone when nothing is active", () => {
    const quiet = [group("a", [agent(10)]), group("b", [agent(20)]), group("c")];
    const { active, rest } = bubbleActive(quiet, NOW);
    expect(active).toEqual([]);
    expect(keys(rest)).toEqual(["a", "b", "c"]);
  });

  it("breaks a tie by manual order", () => {
    const tied = [group("x", [agent(0)]), group("y", [agent(0)]), group("z", [agent(0)])];
    expect(keys(bubbleActive(tied, NOW).active)).toEqual(["x", "y", "z"]);
  });
});
