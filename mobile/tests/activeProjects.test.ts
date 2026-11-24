// Home's project order in src/lib/activeProjects.ts. The ranking itself is
// the desktop's (tests/components/Sidebar/recentProjects.test.ts); this covers
// the mapping from the hosts' snapshots onto it.

import type { AgentRecord, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { describe, expect, it } from "vitest";
import { orderProjects } from "../src/lib/activeProjects";
import { type ProjectEntry, projectEntries } from "../src/lib/projects";

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
  created_at: new Date(NOW - daysAgo * DAY).toISOString(),
});

const ws = (projects: ProjectRef[], agents: AgentRecord[]) =>
  ({ projects, agents, repos: [] }) as Workspace;

const ids = (list: ProjectEntry[]) => list.map((p) => p.project.id);

/** One host's projects, as Home lists them. */
const on = (snapshot: Workspace | null) =>
  projectEntries([{ hostKey: "mini", workspace: snapshot }]);

describe("orderProjects", () => {
  const snapshot = ws(
    [project("a"), project("b"), project("c"), project("d")],
    [agent("a", 30), agent("b", 1), agent("c", 40), agent("d", 0)],
  );

  it("floats projects with recent agents, newest first, over the host's order", () => {
    expect(ids(orderProjects(on(snapshot), {}, NOW))).toEqual(["d", "b", "a", "c"]);
  });

  it("counts a turn sent to an idle agent as activity", () => {
    expect(ids(orderProjects(on(snapshot), { c: NOW }, NOW))).toEqual(["c", "d", "b", "a"]);
  });

  it("ignores archived agents", () => {
    const archived: AgentRecord = {
      ...agent("c", 0),
      archive: { archived_at: "", repos: [], diff_stats: { additions: 0, deletions: 0 } },
    };
    const snap = ws(snapshot.projects, [...snapshot.agents, archived]);
    expect(ids(orderProjects(on(snap), {}, NOW))).toEqual(["d", "b", "a", "c"]);
  });

  it("is the host's order when nothing is recent, and empty without a workspace", () => {
    const quiet = ws([project("a"), project("b")], [agent("a", 10), agent("b", 20)]);
    expect(ids(orderProjects(on(quiet), {}, NOW))).toEqual(["a", "b"]);
    expect(orderProjects(on(null), {}, NOW)).toEqual([]);
  });

  it("floats a project for an agent on any host, after the hosts' own order", () => {
    const entries = projectEntries([
      { hostKey: "mini", workspace: ws([project("a"), project("b")], [agent("a", 10)]) },
      // The studio has `b` too, with yesterday's agent, and a project the mini
      // has not.
      {
        hostKey: "studio",
        workspace: ws([project("b"), project("c")], [agent("b", 1), agent("c", 20)]),
      },
    ]);
    expect(ids(entries)).toEqual(["a", "b", "c"]);
    expect(ids(orderProjects(entries, {}, NOW))).toEqual(["b", "a", "c"]);
  });
});
