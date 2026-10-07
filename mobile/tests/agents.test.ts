// Derivations over the workspace snapshot in src/lib/agents.ts.

import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import { describe, expect, it } from "vitest";
import { agentsOfProject, stageOf } from "../src/lib/agents";

const agent = (over: Partial<AgentRecord>) =>
  ({
    id: "a",
    project_id: "p1",
    created_at: "2026-01-01T00:00:00.000Z",
    repos: [],
    ...over,
  }) as AgentRecord;

const ws = (agents: AgentRecord[]) => ({ agents, projects: [] }) as unknown as Workspace;

describe("agentsOfProject", () => {
  it("returns the project's agents newest first, whatever order the host sent", () => {
    // The host selects `ORDER BY w.created_at` — oldest first.
    const list = agentsOfProject(
      ws([
        agent({ id: "old", created_at: "2026-01-01T00:00:01.000Z" }),
        agent({ id: "mid", created_at: "2026-01-01T00:00:02.000Z" }),
        agent({ id: "new", created_at: "2026-01-01T00:00:03.000Z" }),
      ]),
      "p1",
    );
    expect(list.map((a) => a.id)).toEqual(["new", "mid", "old"]);
  });

  it("breaks a same-millisecond tie on id, so the order is total", () => {
    const tied = ["c", "a", "b"].map((id) => agent({ id, created_at: "2026-01-01T00:00:01.000Z" }));
    const list = agentsOfProject(
      ws([...tied, agent({ id: "newer", created_at: "2026-01-01T00:00:02.000Z" })]),
      "p1",
    );
    expect(list.map((a) => a.id)).toEqual(["newer", "a", "b", "c"]);
  });

  it("keeps out other projects' agents and archived ones", () => {
    const list = agentsOfProject(
      ws([
        agent({ id: "mine" }),
        agent({ id: "theirs", project_id: "p2" }),
        agent({
          id: "archived",
          archive: {
            archived_at: "2026-01-02T00:00:00.000Z",
            repos: [],
            diff_stats: { additions: 0, deletions: 0 },
          },
        }),
      ]),
      "p1",
    );
    expect(list.map((a) => a.id)).toEqual(["mine"]);
  });

  it("is empty with no snapshot", () => {
    expect(agentsOfProject(null, "p1")).toEqual([]);
  });
});

describe("stageOf", () => {
  it("puts a turn in flight under running, whatever its PR", () => {
    expect(stageOf(agent({ status: "running" }), "open")).toBe("running");
    expect(stageOf(agent({ status: "spawning" }), null)).toBe("running");
  });

  it("hands a resting agent without a PR back to the user", () => {
    expect(stageOf(agent({ status: "idle" }), null)).toBe("yours");
    expect(stageOf(agent({ status: "error" }), null)).toBe("yours");
    expect(stageOf(agent({ status: "stopped" }), null)).toBe("yours");
  });

  it("hands an errored agent back to the user even with a PR", () => {
    expect(stageOf(agent({ status: "error" }), "open")).toBe("yours");
  });

  it("files a resting agent with a PR under PRs, whatever its state", () => {
    expect(stageOf(agent({ status: "idle" }), "open")).toBe("prs");
    expect(stageOf(agent({ status: "idle" }), "merged")).toBe("prs");
    expect(stageOf(agent({ status: "idle" }), "closed")).toBe("prs");
  });
});
