// The sidebar's project groups over every host: This Mac's projects, each with
// the work of every paired host that runs in it, then the projects only a
// paired host has. Pure: the hosts' snapshots in, the groups out.
//
// A project and its copies on other hosts share a `project_id` by design, so
// id equality is the whole match between hosts. A path names a folder on one
// host only, so it never matches across them.

import type { AgentRecord, WfRun, Workspace } from "@/api";
import type { DraftAgent } from "@/store";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { basename } from "@/util/format";
import { groupProjectRefs, projectKeyResolver } from "@/util/projects";
import {
  type ProjectSignal,
  projectSignal,
  type SignalSources,
  strongestSignal,
} from "./projectSignal";

/** One host's part of the sidebar. `workspace` is null until it first loads. */
export interface HostSnapshot {
  hostId: EnvironmentId;
  workspace: Pick<Workspace, "projects" | "agents"> | null;
  drafts: readonly DraftAgent[];
  runs: readonly WfRun[];
}

/** A sidebar row: a workspace, a draft or a workflow run. */
export type SidebarRow = AgentRecord | DraftAgent | WfRun;

export interface ProjectGroupData {
  /** Stable group id: the project_id, or the repo path for repos that aren't
   *  pinned (an agent whose repo was removed from the sidebar). A paired
   *  host's path is prefixed with the host, since it names nothing here. */
  key: string;
  /** Project display name (folder basename fallback for unpinned repos). */
  label: string;
  /** All repos attached to the project, in creation order, on `hostId`. */
  repoPaths: string[];
  /** The project's first repo on `hostId`: the drag/order key, draft target,
   *  and settings key, so single-repo projects behave exactly as before. */
  primaryPath: string;
  /** The host the group's header acts on: This Mac for every project it has,
   *  else the first paired host (This Mac first, then by name) that has it. */
  hostId: EnvironmentId;
  /** Newest first, every host's together. */
  agents: AgentRecord[];
  drafts: DraftAgent[];
  /** Newest-updated first, every host's together. */
  runs: WfRun[];
  /** The host each row runs on. Rows are the hosts' own records, so this is
   *  keyed by identity: two hosts' agents of one id are two rows. */
  hostOf: ReadonlyMap<SidebarRow, EnvironmentId>;
  pinned: boolean;
}

/** The host `row` of group `g` runs on. */
export const rowHost = (g: Pick<ProjectGroupData, "hostOf">, row: SidebarRow): EnvironmentId =>
  g.hostOf.get(row) ?? LOCAL_ENVIRONMENT_ID;

/** Build one group per project, This Mac's first (see `addHost` for how one
 *  host's snapshot lands), then each paired host's in turn. */
export function groupByProject(
  local: HostSnapshot,
  hosts: readonly HostSnapshot[],
): ProjectGroupData[] {
  const groups = new Map<string, ProjectGroupData>();
  const hostOf = new Map<SidebarRow, EnvironmentId>();
  for (const snapshot of [local, ...hosts]) addHost(groups, hostOf, snapshot);
  // Each host's rows arrive in order; a stable sort merges the hosts' lists
  // and leaves a tie in host order (This Mac's first).
  for (const g of groups.values()) {
    g.agents.sort((a, b) =>
      a.created_at < b.created_at ? 1 : a.created_at > b.created_at ? -1 : 0,
    );
    g.runs.sort((a, b) => b.updated_at - a.updated_at);
  }
  return [...groups.values()];
}

/** One host's projects and work, into `groups`.
 *
 *  The pinned repos (`workspace.projects`) are the single source of truth for
 *  which groups exist: a multi-repo project folds into one group, keyed by
 *  `project_id`, and a project another host already brought is the same
 *  group. Agents, runs, and drafts only *attach* to those groups — they never
 *  fabricate one from a stale reference, so a deleted or relocated project
 *  can't reappear as a phantom sidebar group.
 *
 *  Agents and runs resolve by `project_id` first, across hosts (it survives a
 *  relocate, where a run's stored `repo_path` and a draft's path both go
 *  stale), then by the host's own repo path, and only a genuinely *unpinned*
 *  repo (e.g. detached while an agent still runs in it) gets a path-keyed
 *  fallback group. Drafts carry no project_id, so they attach by the host's
 *  path only and are dropped from the view when their repo is gone. */
function addHost(
  groups: Map<string, ProjectGroupData>,
  hostOf: Map<SidebarRow, EnvironmentId>,
  { hostId, workspace, drafts, runs }: HostSnapshot,
) {
  const refs = workspace?.projects ?? [];
  const projectIds = new Set(refs.map((r) => r.project_id).filter(Boolean));
  // A key from this host's own resolution: a project id is shared by design,
  // a path is this host's alone.
  const own = (key: string) =>
    hostId === LOCAL_ENVIRONMENT_ID || projectIds.has(key) ? key : `${hostId}:${key}`;
  const byPath = new Map<string, ProjectGroupData>();
  const add = (key: string, label: string, repoPaths: string[], pinned: boolean) => {
    const g: ProjectGroupData = {
      key,
      label,
      repoPaths,
      primaryPath: repoPaths[0],
      hostId,
      agents: [],
      drafts: [],
      runs: [],
      hostOf,
      pinned,
    };
    groups.set(key, g);
    return g;
  };

  for (const project of groupProjectRefs(refs)) {
    const key = own(project.id);
    const paths = project.repos.map((r) => r.path);
    const g = groups.get(key) ?? add(key, project.name, paths, true);
    for (const path of paths) byPath.set(path, g);
  }

  const keyOf = projectKeyResolver(refs);
  const groupFor = (projectId: string, path: string | undefined): ProjectGroupData | null => {
    const shared = projectId ? groups.get(projectId) : undefined;
    if (shared) return shared;
    const resolved = keyOf(projectId, path);
    if (resolved === undefined) return null;
    const key = own(resolved);
    const found = groups.get(key);
    if (found) return found;
    const g = add(key, basename(resolved), [resolved], false);
    byPath.set(resolved, g);
    return g;
  };
  const attach = <T extends SidebarRow>(
    row: T,
    list: (g: ProjectGroupData) => T[],
    g: ProjectGroupData | null | undefined,
  ) => {
    if (!g) return;
    list(g).push(row);
    hostOf.set(row, hostId);
  };

  // The live workspaces, newest first, as the sidebar has always ordered them.
  const live = (workspace?.agents ?? [])
    .filter((a) => !a.archive)
    .sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
  for (const a of live) attach(a, (g) => g.agents, groupFor(a.project_id, a.repos[0]?.repo_path));
  for (const r of runs) attach(r, (g) => g.runs, groupFor(r.project_id, r.repo_path));
  for (const d of drafts) attach(d, (g) => g.drafts, byPath.get(d.repoPath));
}

/** The strongest signal among a group's rows, each host's read against its
 *  own maps (`sourcesOf`), the way that host's rows read them. */
export function groupSignal(
  g: ProjectGroupData,
  sourcesOf: (hostId: EnvironmentId) => SignalSources,
): ProjectSignal | null {
  const hosts = new Set([...g.agents, ...g.runs].map((row) => rowHost(g, row)));
  return strongestSignal(
    [...hosts].map((hostId) =>
      projectSignal(
        {
          agents: g.agents.filter((a) => rowHost(g, a) === hostId),
          runs: g.runs.filter((r) => rowHost(g, r) === hostId),
        },
        sourcesOf(hostId),
      ),
    ),
  );
}
