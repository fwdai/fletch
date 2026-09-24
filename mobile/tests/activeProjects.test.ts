// Home's project order in src/lib/activeProjects.ts. The ranking itself is
// the desktop's (tests/components/Sidebar/recentProjects.test.ts); this covers
// the mapping from a workspace snapshot onto it.

import type { AgentRecord, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { describe, expect, it } from "vitest";
import { orderProjects } from "../src/lib/activeProjects";

const DAY = 86_400_000;
const NOW = Date.parse("2026-09-24T12:00:00Z");

const project = (id: string) =>
  ({ project_id: id, path: `/${id}`, name: id, label: null }) as ProjectRef;

const agent = (
  projectId: string,
  daysAgo: number,
  status: AgentRecord["status"] = "idle",
): AgentRecord => ({
  id: `${projectId}-${daysAgo}`,
  project_id: projectId,
  name: "a",
  provider: "claude",
  repos: [],
  task: "",
  status,
  view: "custom",
  created_at: new Date(NOW - daysAgo * DAY).toISOString(),
});

const ws = (projects: ProjectRef[], agents: AgentRecord[]) =>
  ({ projects, agents, repos: [] }) as Workspace;

const ids = (list: ProjectRef[]) => list.map((p) => p.project_id);

describe("orderProjects", () => {
  const snapshot = ws(
    [project("a"), project("b"), project("c"), project("d")],
    [agent("a", 30), agent("b", 1), agent("c", 40), agent("d", 0)],
  );

  it("floats projects with recent agents, newest first, over the host's order", () => {
    expect(ids(orderProjects(snapshot, {}, NOW))).toEqual(["d", "b", "a", "c"]);
  });

  it("counts a turn sent to an idle agent as activity", () => {
    expect(ids(orderProjects(snapshot, { c: NOW }, NOW))).toEqual(["c", "d", "b", "a"]);
  });

  it("ignores archived agents", () => {
    const archived: AgentRecord = {
      ...agent("c", 0),
      archive: { archived_at: "", repos: [], diff_stats: { additions: 0, deletions: 0 } },
    };
    const snap = ws(snapshot.projects, [...snapshot.agents, archived]);
    expect(ids(orderProjects(snap, {}, NOW))).toEqual(["d", "b", "a", "c"]);
  });

  it("is the host's order when nothing is recent, and empty without a workspace", () => {
    const quiet = ws([project("a"), project("b")], [agent("a", 10), agent("b", 20)]);
    expect(ids(orderProjects(quiet, {}, NOW))).toEqual(["a", "b"]);
    expect(orderProjects(null, {}, NOW)).toEqual([]);
  });
});
