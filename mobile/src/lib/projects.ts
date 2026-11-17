// The phone's view of a project. The host snapshot lists repos, not projects
// (`Workspace.projects` is one `ProjectRef` per repo), so every surface that
// shows, picks or counts a project reads the folded list from here — never
// `workspace.projects` directly — and a multi-repo project is one project
// everywhere, with one card, one picker row and one id. Every paired host's
// lists are merged here too (`projectEntries`): a project several hosts have
// is still one card.

import type { Workspace } from "@desktop/api/types/agent";
import { groupProjectRefs, type ProjectGroup } from "@desktop/util/projects";
import { agentsOfProject, type HostAgent, newestFirst } from "./agents";

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

/** One paired host's snapshot, as the merged list reads it. */
export interface HostSnapshot {
  hostKey: string;
  workspace: Workspace | null;
}

/** A project as one host has it. */
export interface HostProject {
  hostKey: string;
  project: Project;
}

/** A project as Home lists it: one entry for every paired host that has it. */
export interface ProjectEntry {
  /** What tells it from every other entry: its project id, which every host
   *  that has the project shares — or, for a repo whose project row is
   *  missing, its path, prefixed with the one host it names a folder on. */
  key: string;
  /** The host that speaks for it (`speakerOf`), and the project as that host
   *  has it: what its name, icon and repo label are read from. */
  hostKey: string;
  project: Project;
  /** Every host that has it, in saved order. */
  hosts: HostProject[];
  /** Those hosts' live agents in it, all together, newest first. */
  agents: HostAgent[];
}

/** Whether `project` is its host's own. A host that only runs it holds a copy,
 *  whose repos name the project's home. */
const isHome = (project: Project) => project.repos.every((r) => !r.home);

/** The host that speaks for a project wherever one host's data is wanted — its
 *  name, icon and repos, its roadmap and its planning chats: the project's
 *  home, else the first host that has it. `hosts` is in saved order. */
export const speakerOf = <T extends HostProject>(hosts: readonly T[]): T | undefined =>
  hosts.find((h) => isHome(h.project)) ?? hosts[0];

/** Every paired host's projects as one list, merged the way the desktop
 *  sidebar merges its hosts' (`groupByProject`, src/components/Sidebar/
 *  hostGroups.ts): a project id is shared by every host that has the project,
 *  while a path names a folder on one host only, so a project keyed by its
 *  path is never merged. In the order the hosts list them, the first host's
 *  first. `snapshots` is in saved order. */
export function projectEntries(snapshots: readonly HostSnapshot[]): ProjectEntry[] {
  const byKey = new Map<string, HostProject[]>();
  for (const { hostKey, workspace } of snapshots) {
    for (const project of projectsOf(workspace)) {
      const key = project.repos[0].project_id ? project.id : `${hostKey}:${project.id}`;
      const hosts = byKey.get(key);
      if (hosts) hosts.push({ hostKey, project });
      else byKey.set(key, [{ hostKey, project }]);
    }
  }
  const workspaceOf = new Map(snapshots.map((s) => [s.hostKey, s.workspace]));
  return [...byKey].map(([key, hosts]) => {
    const { hostKey, project } = speakerOf(hosts) ?? hosts[0];
    const agents = hosts
      .flatMap((h) =>
        agentsOfProject(workspaceOf.get(h.hostKey), h.project.id).map((agent) => ({
          hostKey: h.hostKey,
          agent,
        })),
      )
      // Stable, so an agent id two hosts created at the same moment keeps the
      // hosts' order.
      .sort((a, b) => newestFirst(a.agent, b.agent));
    return { key, hostKey, project, hosts, agents };
  });
}

/** The entry `hostKey`'s project `projectId` is part of: how a screen or sheet
 *  opened on a host's project finds every host's share of it. */
export const entryOf = (
  entries: readonly ProjectEntry[],
  hostKey: string | null,
  projectId: string | undefined,
): ProjectEntry | undefined =>
  entries.find((e) => e.hosts.some((h) => h.hostKey === hostKey && h.project.id === projectId));
