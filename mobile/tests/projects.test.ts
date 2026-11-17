// The phone's project list in src/lib/projects.ts: the host's per-repo refs
// folded into one project per id, which is what keeps a multi-repo project
// from showing up once per repo on Home (and in every picker and count) — and
// every paired host's lists merged, so a project several hosts have is still
// one.

import type { AgentRecord, ProjectRef, Workspace } from "@desktop/api/types/agent";
import { describe, expect, it } from "vitest";
import { hostAgentKey } from "../src/lib/agents";
import {
  entryOf,
  type HostSnapshot,
  primaryPath,
  projectById,
  projectEntries,
  projectsOf,
  repoLabel,
  speakerOf,
} from "../src/lib/projects";

const ref = (project_id: string, path: string, name: string, label: string | null = null) =>
  ({ project_id, path, name, label }) as ProjectRef;

const ws = (projects: ProjectRef[], agents: AgentRecord[] = []) =>
  ({ projects, agents, repos: [] }) as Workspace;

describe("projectsOf", () => {
  const snapshot = ws([
    ref("eve", "/code/assistant", "Eve"),
    ref("fletch", "/code/fletch", "Fletch"),
    ref("eve", "/code/eve", "Eve"),
  ]);

  it("folds a multi-repo project into one entry, in the host's order", () => {
    const projects = projectsOf(snapshot);
    expect(projects.map((p) => p.id)).toEqual(["eve", "fletch"]);
    expect(projects[0].repos.map((r) => r.path)).toEqual(["/code/assistant", "/code/eve"]);
  });

  it("spawns in the first repo and labels the card with all of them", () => {
    const eve = projectById(snapshot, "eve");
    expect(eve && primaryPath(eve)).toBe("/code/assistant");
    expect(eve && repoLabel(eve)).toBe("assistant · eve");
  });

  it("prefers a repo's own label over its folder name", () => {
    const labelled = ws([ref("p", "/code/web", "App", "Frontend"), ref("p", "/code/api", "App")]);
    expect(repoLabel(projectsOf(labelled)[0])).toBe("Frontend · api");
  });

  it("keeps a repo whose project row is missing, keyed by its path", () => {
    const orphan = ws([ref("", "/code/loose", "loose")]);
    expect(projectsOf(orphan).map((p) => p.id)).toEqual(["/code/loose"]);
  });

  it("is the same list for the same snapshot, so selectors stay stable", () => {
    expect(projectsOf(snapshot)).toBe(projectsOf(snapshot));
    expect(projectById(snapshot, "eve")).toBe(projectById(snapshot, "eve"));
    expect(projectsOf(null)).toBe(projectsOf(null));
    expect(projectById(snapshot, undefined)).toBeUndefined();
  });
});

const agent = (
  id: string,
  project_id: string,
  created_at: string,
  over: Partial<AgentRecord> = {},
) => ({ id, project_id, created_at, name: id, repos: [], status: "idle", ...over }) as AgentRecord;

/** A project as a host that only runs it holds it: a copy, naming its home. */
const copyOf = (r: ProjectRef, home: string): ProjectRef => ({
  ...r,
  home: { device: home, name: home },
});

describe("projectEntries", () => {
  const fletch = ref("fletch", "/Users/alex/code/fletch", "fletch");
  const atlas = ref("atlas", "/Users/alex/code/atlas", "atlas");
  const relay = ref("relay", "/Users/sam/relay", "relay");
  const mini: HostSnapshot = {
    hostKey: "mini",
    workspace: ws(
      [fletch, atlas],
      [
        agent("arabia", "fletch", "2026-01-01T00:00:02Z"),
        agent("caspian", "atlas", "2026-01-01T00:00:05Z"),
      ],
    ),
  };
  const studio: HostSnapshot = {
    hostKey: "studio",
    workspace: ws(
      [{ ...fletch, path: "/Volumes/work/fletch" }, relay],
      [
        // The same id as the mini's, from the same moment: another agent.
        agent("arabia", "fletch", "2026-01-01T00:00:02Z"),
        agent("lofoten", "fletch", "2026-01-01T00:00:03Z"),
        agent("gone", "fletch", "2026-01-01T00:00:09Z", {
          archive: { archived_at: "", repos: [], diff_stats: { additions: 0, deletions: 0 } },
        }),
      ],
    ),
  };

  it("is one entry per project, merged by id, with every host's agents newest first", () => {
    const entries = projectEntries([mini, studio]);
    expect(entries.map((e) => e.key)).toEqual(["fletch", "atlas", "relay"]);
    const [shared] = entries;
    expect(shared.hosts.map((h) => h.hostKey)).toEqual(["mini", "studio"]);
    // Each host keeps its own path: a folder is one host's.
    expect(shared.hosts.map((h) => primaryPath(h.project))).toEqual([
      "/Users/alex/code/fletch",
      "/Volumes/work/fletch",
    ]);
    // Archived agents are left out; the two `arabia`s are two rows, the tie
    // in saved order.
    expect(shared.agents.map(hostAgentKey)).toEqual([
      "studio:lofoten",
      "mini:arabia",
      "studio:arabia",
    ]);
  });

  it("lists a project only one host has, with that host's agents alone", () => {
    const entries = projectEntries([mini, studio]);
    const onMini = entries.find((e) => e.key === "atlas");
    expect(onMini?.hostKey).toBe("mini");
    expect(onMini?.agents.map(hostAgentKey)).toEqual(["mini:caspian"]);
    expect(entries.find((e) => e.key === "relay")?.hostKey).toBe("studio");
  });

  it("never merges by path: a repo with no project row stays its own host's", () => {
    const loose = ref("", "/code/loose", "loose");
    const entries = projectEntries([
      { hostKey: "mini", workspace: ws([loose]) },
      { hostKey: "studio", workspace: ws([loose]) },
    ]);
    expect(entries.map((e) => [e.key, e.hostKey])).toEqual([
      ["mini:/code/loose", "mini"],
      ["studio:/code/loose", "studio"],
    ]);
  });

  it("is the one host's list, in its order, with one host", () => {
    const entries = projectEntries([mini]);
    expect(entries.map((e) => e.project)).toEqual(projectsOf(mini.workspace));
    expect(entries.every((e) => e.hostKey === "mini")).toBe(true);
    expect(projectEntries([{ hostKey: "mini", workspace: null }])).toEqual([]);
  });
});

describe("the host that speaks for a project", () => {
  const fletch = ref("fletch", "/code/fletch", "fletch");

  it("is its home, wherever that is in the saved order", () => {
    const [entry] = projectEntries([
      { hostKey: "mini", workspace: ws([copyOf(fletch, "studio")]) },
      { hostKey: "studio", workspace: ws([fletch]) },
    ]);
    expect(entry.hostKey).toBe("studio");
    expect(entry.project.repos[0].home).toBeUndefined();
  });

  it("is the first paired host that has it when none of them is its home", () => {
    const [entry] = projectEntries([
      { hostKey: "mini", workspace: ws([copyOf(fletch, "laptop")]) },
      { hostKey: "studio", workspace: ws([copyOf(fletch, "laptop")]) },
    ]);
    expect(entry.hostKey).toBe("mini");
    // …which is also how hosts from before `home` read: each as the owner.
    const own = projectsOf(ws([fletch]))[0];
    expect(
      speakerOf([
        { hostKey: "mini", project: own },
        { hostKey: "studio", project: own },
      ])?.hostKey,
    ).toBe("mini");
    expect(speakerOf([])).toBeUndefined();
  });
});

describe("entryOf", () => {
  const entries = projectEntries([
    { hostKey: "mini", workspace: ws([ref("fletch", "/code/fletch", "fletch")]) },
    { hostKey: "studio", workspace: ws([ref("fletch", "/code/fletch", "fletch")]) },
  ]);

  it("finds a project's entry from any host that has it", () => {
    expect(entryOf(entries, "mini", "fletch")).toBe(entries[0]);
    expect(entryOf(entries, "studio", "fletch")).toBe(entries[0]);
    expect(entryOf(entries, "laptop", "fletch")).toBeUndefined();
    expect(entryOf(entries, "mini", undefined)).toBeUndefined();
  });
});
