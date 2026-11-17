import { describe, expect, it } from "vitest";
import type { AgentRecord, ProjectRef, WfRun } from "@/api";
import type { DraftAgent } from "@/store";
import { LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { groupByProject, groupSignal, type HostSnapshot, rowHost } from "./hostGroups";
import type { SignalSources } from "./projectSignal";

const project = (path: string, project_id: string, name = path.split("/").pop() ?? path) =>
  ({ path, name, project_id, label: null }) as ProjectRef;

const agent = (id: string, project_id: string, repo: string, over: Partial<AgentRecord> = {}) =>
  ({
    id,
    project_id,
    name: id,
    provider: "claude",
    repos: [{ repo_path: repo, subdir: "", branch: `feat/${id}` }],
    task: `Task for ${id}`,
    status: "idle",
    view: "custom",
    created_at: "2026-11-05T10:00:00Z",
    ...over,
  }) as AgentRecord;

const snapshot = (
  hostId: string,
  projects: ProjectRef[],
  agents: AgentRecord[] = [],
  extra: Partial<HostSnapshot> = {},
): HostSnapshot => ({
  hostId,
  workspace: { projects, agents },
  drafts: [],
  runs: [],
  ...extra,
});

const local = (projects: ProjectRef[], agents: AgentRecord[] = [], extra = {}) =>
  snapshot(LOCAL_ENVIRONMENT_ID, projects, agents, extra);

/** Each group as `key: [host id, agent id]…`, for a compact comparison. */
const shape = (groups: ReturnType<typeof groupByProject>) =>
  Object.fromEntries(
    groups.map((g) => [g.key, g.agents.map((a) => [rowHost(g, a), a.id] as const)]),
  );

describe("groupByProject", () => {
  it("with no paired host, builds This Mac's groups alone, newest first", () => {
    const groups = groupByProject(
      local(
        [project("/w/fletch", "p1"), project("/w/relay", "p2")],
        [
          agent("old", "p1", "/w/fletch", { created_at: "2026-11-01T00:00:00Z" }),
          agent("new", "p1", "/w/fletch", { created_at: "2026-11-05T00:00:00Z" }),
          agent("gone", "p2", "/w/relay", {
            archive: {
              archived_at: "2026-11-04T00:00:00Z",
              repos: [],
              diff_stats: { additions: 0, deletions: 0 },
            },
          }),
        ],
      ),
      [],
    );

    expect(groups.map((g) => [g.key, g.label, g.primaryPath, g.hostId, g.pinned])).toEqual([
      ["p1", "fletch", "/w/fletch", LOCAL_ENVIRONMENT_ID, true],
      ["p2", "relay", "/w/relay", LOCAL_ENVIRONMENT_ID, true],
    ]);
    // Archived workspaces are History's, not the sidebar's.
    expect(shape(groups)).toEqual({
      p1: [
        [LOCAL_ENVIRONMENT_ID, "new"],
        [LOCAL_ENVIRONMENT_ID, "old"],
      ],
      p2: [],
    });
  });

  it("puts a paired host's work in this Mac's project of the same id, by id alone", () => {
    const groups = groupByProject(
      local([project("/w/fletch", "p1")], [agent("fuji", "p1", "/w/fletch")]),
      [
        // The host's copy lives at another path, and is named otherwise.
        snapshot(
          "studio",
          [project("/srv/code/fletch-copy", "p1", "Fletch on the studio")],
          [agent("etna", "p1", "/srv/code/fletch-copy", { created_at: "2026-11-06T00:00:00Z" })],
        ),
      ],
    );

    expect(groups).toHaveLength(1);
    // The group is this Mac's: its name, its path, its header.
    expect(groups[0]).toMatchObject({
      key: "p1",
      label: "fletch",
      primaryPath: "/w/fletch",
      hostId: LOCAL_ENVIRONMENT_ID,
    });
    // Ordered newest first over both hosts' rows.
    expect(shape(groups).p1).toEqual([
      ["studio", "etna"],
      [LOCAL_ENVIRONMENT_ID, "fuji"],
    ]);
  });

  it("never matches across hosts by path or name", () => {
    const groups = groupByProject(local([project("/w/fletch", "p1")]), [
      snapshot(
        "studio",
        [project("/w/fletch", "p7", "fletch")],
        [agent("etna", "p7", "/w/fletch")],
      ),
    ]);

    expect(groups.map((g) => [g.key, g.hostId])).toEqual([
      ["p1", LOCAL_ENVIRONMENT_ID],
      ["p7", "studio"],
    ]);
    expect(shape(groups).p1).toEqual([]);
  });

  it("gives a project only a paired host has a group of its own, after this Mac's", () => {
    const groups = groupByProject(local([project("/w/fletch", "p1")]), [
      snapshot(
        "studio",
        [project("/srv/relay", "p9", "Relay"), project("/srv/empty", "p8", "Empty")],
        [agent("etna", "p9", "/srv/relay")],
      ),
    ]);

    expect(groups.map((g) => [g.key, g.label, g.primaryPath, g.hostId])).toEqual([
      ["p1", "fletch", "/w/fletch", LOCAL_ENVIRONMENT_ID],
      ["p9", "Relay", "/srv/relay", "studio"],
      // A host's project with nothing in it is listed too, as this Mac's are.
      ["p8", "Empty", "/srv/empty", "studio"],
    ]);
    expect(shape(groups).p9).toEqual([["studio", "etna"]]);
  });

  it("makes one group of a project two paired hosts share, headed by the first", () => {
    const groups = groupByProject(local([]), [
      snapshot(
        "cloud",
        [project("/home/relay", "p9", "Relay")],
        [agent("etna", "p9", "/home/relay")],
      ),
      snapshot(
        "studio",
        [project("/srv/relay", "p9", "relay")],
        [agent("fuji", "p9", "/srv/relay")],
      ),
    ]);

    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ key: "p9", label: "Relay", hostId: "cloud" });
    expect(
      shape(groups)
        .p9.map(([host]) => host)
        .sort(),
    ).toEqual(["cloud", "studio"]);
  });

  it("keeps an agent id two hosts both have as two rows, each tagged with its host", () => {
    const groups = groupByProject(
      local([project("/w/fletch", "p1")], [agent("fuji", "p1", "/w/fletch")]),
      [
        snapshot("studio", [project("/srv/fletch", "p1")], [agent("fuji", "p1", "/srv/fletch")]),
        snapshot("cloud", [], [agent("fuji", "p1", "/home/fletch")]),
      ],
    );

    const rows = shape(groups).p1;
    expect(rows).toHaveLength(3);
    expect(rows.map(([host]) => host)).toEqual([LOCAL_ENVIRONMENT_ID, "studio", "cloud"]);
    expect(rows.every(([, id]) => id === "fuji")).toBe(true);
  });

  it("keeps a paired host's unpinned repo apart from this Mac's of the same path", () => {
    const groups = groupByProject(local([], [agent("fuji", "gone", "/w/scratch")]), [
      snapshot("studio", [], [agent("etna", "gone-too", "/w/scratch")]),
    ]);

    expect(groups.map((g) => [g.key, g.label, g.hostId, g.pinned])).toEqual([
      ["/w/scratch", "scratch", LOCAL_ENVIRONMENT_ID, false],
      ["studio:/w/scratch", "scratch", "studio", false],
    ]);
  });

  it("attaches each host's drafts and runs by its own paths and ids", () => {
    const draft = (id: string, repoPath: string) => ({ id, repoPath, name: id }) as DraftAgent;
    const run = (id: string, project_id: string, repo_path: string, updated_at: number) =>
      ({ id, project_id, repo_path, updated_at, status: "running" }) as WfRun;
    const groups = groupByProject(
      local([project("/w/fletch", "p1")], [], {
        drafts: [draft("d-mine", "/w/fletch")],
        runs: [run("r-mine", "p1", "/w/fletch", 1)],
      }),
      [
        snapshot("studio", [project("/srv/fletch", "p1")], [], {
          // A draft names a path on its own host; this Mac's path means nothing there.
          drafts: [draft("d-theirs", "/srv/fletch"), draft("d-lost", "/w/fletch")],
          runs: [run("r-theirs", "p1", "/srv/fletch", 2)],
        }),
      ],
    );

    const [g] = groups;
    expect(g.drafts.map((d) => [rowHost(g, d), d.id])).toEqual([
      [LOCAL_ENVIRONMENT_ID, "d-mine"],
      ["studio", "d-theirs"],
    ]);
    // Newest-updated first, over both hosts.
    expect(g.runs.map((r) => [rowHost(g, r), r.id])).toEqual([
      ["studio", "r-theirs"],
      [LOCAL_ENVIRONMENT_ID, "r-mine"],
    ]);
  });
});

describe("groupSignal", () => {
  it("reads each host's rows against that host's own maps", () => {
    // This Mac's `fuji` is working; the studio's, of the same id, is idle.
    const groups = groupByProject(
      local(
        [project("/w/fletch", "p1")],
        [agent("fuji", "p1", "/w/fletch", { status: "running" })],
      ),
      [snapshot("studio", [project("/srv/fletch", "p1")], [agent("fuji", "p1", "/srv/fletch")])],
    );
    const signal = (sources: Record<string, SignalSources>) =>
      groupSignal(groups[0], (hostId) => sources[hostId]);

    // The studio's idle `fuji` finished unseen.
    expect(
      signal({
        [LOCAL_ENVIRONMENT_ID]: { pending: {}, unseen: {} },
        studio: { pending: {}, unseen: { fuji: true } },
      }),
    ).toBe("unseen");
    // This Mac's map naming `fuji` says nothing about the studio's row.
    expect(
      signal({
        [LOCAL_ENVIRONMENT_ID]: { pending: {}, unseen: { fuji: true } },
        studio: { pending: {}, unseen: {} },
      }),
    ).toBe("running");
  });
});
