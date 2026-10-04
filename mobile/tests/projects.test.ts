// The phone's project list in src/lib/projects.ts: the host's per-repo refs
// folded into one project per id, which is what keeps a multi-repo project
// from showing up once per repo on Home (and in every picker and count).

import type { ProjectRef, Workspace } from "@desktop/api/types/agent";
import { describe, expect, it } from "vitest";
import { primaryPath, projectById, projectsOf, repoLabel } from "../src/lib/projects";

const ref = (project_id: string, path: string, name: string, label: string | null = null) =>
  ({ project_id, path, name, label }) as ProjectRef;

const ws = (projects: ProjectRef[]) => ({ projects, agents: [], repos: [] }) as Workspace;

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
