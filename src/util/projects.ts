import type { ProjectRef } from "@/api/types/agent";

/** One project, with every repo the host has pinned under it.
 *
 *  The host's `workspace.projects` is a list of *repos* — one `ProjectRef`
 *  per repo, sharing a `project_id` when a project has several — so anything
 *  that renders "a project" must fold that list first. This is the folded
 *  shape both apps read from. */
export interface ProjectGroup {
  /** The host's `project_id`, or the repo path for a repo whose project row is
   *  missing (the host then names it after the folder). */
  id: string;
  name: string;
  /** The project's repos in the host's order; `repos[0]` is the primary — the
   *  one a single-repo surface spawns in. Never empty. */
  repos: ProjectRef[];
}

/** What a repo's project is keyed by: the host's `project_id`, or the repo
 *  path where the project row is missing. */
export const projectKey = (ref: ProjectRef): string => ref.project_id || ref.path;

/** Which project a piece of work (an agent, a run, a repo in front) belongs
 *  to, as a key: by `project_id` first, so a relocate — which moves the repo
 *  path but keeps the id — still lands on the moved project; then by a pinned
 *  repo path; else the path itself, since a repo that isn't pinned is its own
 *  group wherever projects are listed. Undefined with neither to go by. */
export function projectKeyResolver(
  refs: readonly ProjectRef[],
): (projectId: string | undefined, path: string | undefined) => string | undefined {
  const byId = new Map<string, string>();
  const byPath = new Map<string, string>();
  for (const group of groupProjectRefs(refs)) {
    if (group.repos[0].project_id) byId.set(group.repos[0].project_id, group.id);
    for (const r of group.repos) byPath.set(r.path, group.id);
  }
  return (projectId, path) =>
    (projectId ? byId.get(projectId) : undefined) ??
    (path ? (byPath.get(path) ?? path) : undefined);
}

/** Fold per-repo refs into one group per project, in first-seen order. */
export function groupProjectRefs(refs: readonly ProjectRef[]): ProjectGroup[] {
  const groups = new Map<string, ProjectGroup>();
  for (const ref of refs) {
    const id = projectKey(ref);
    let group = groups.get(id);
    if (!group) {
      group = { id, name: ref.name, repos: [] };
      groups.set(id, group);
    }
    group.repos.push(ref);
  }
  return [...groups.values()];
}
