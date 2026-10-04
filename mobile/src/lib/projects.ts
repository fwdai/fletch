// The phone's view of a project. The host snapshot lists repos, not projects
// (`Workspace.projects` is one `ProjectRef` per repo), so every surface that
// shows, picks or counts a project reads the folded list from here — never
// `workspace.projects` directly — and a multi-repo project is one project
// everywhere, with one card, one picker row and one id.

import type { Workspace } from "@desktop/api/types/agent";
import { groupProjectRefs, type ProjectGroup } from "@desktop/util/projects";

export type Project = ProjectGroup;

const NONE: Project[] = [];
// One fold per snapshot: the store replaces the workspace object on every
// read, and zustand selectors need a stable reference back, so the folded
// list is kept against the snapshot it came from.
const folded = new WeakMap<Workspace, Project[]>();

/** The host's projects, one per project id, in the host's order. */
export function projectsOf(ws: Workspace | null): Project[] {
  if (!ws) return NONE;
  let list = folded.get(ws);
  if (!list) {
    list = groupProjectRefs(ws.projects);
    folded.set(ws, list);
  }
  return list;
}

export const projectById = (ws: Workspace | null, id: string | undefined) =>
  id === undefined ? undefined : projectsOf(ws).find((p) => p.id === id);

/** The repo an agent started from the phone is spawned in: the phone is
 *  single-repo for now, so this is the project's primary repo. */
export const primaryPath = (project: Project) => project.repos[0].path;

/** The project's repos by folder name, "assistant · eve" for a multi-repo one. */
export const repoLabel = (project: Project) =>
  project.repos.map((r) => r.label ?? (r.path.split("/").pop() || project.name)).join(" · ");

/** A stable hue per project so its swatch keeps its colour across launches. */
export function projectHue(project: Project): number {
  let h = 0;
  for (const ch of project.id) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}
