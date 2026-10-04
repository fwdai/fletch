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

/** Fold per-repo refs into one group per project, in first-seen order. */
export function groupProjectRefs(refs: readonly ProjectRef[]): ProjectGroup[] {
  const groups = new Map<string, ProjectGroup>();
  for (const ref of refs) {
    const id = ref.project_id || ref.path;
    let group = groups.get(id);
    if (!group) {
      group = { id, name: ref.name, repos: [] };
      groups.set(id, group);
    }
    group.repos.push(ref);
  }
  return [...groups.values()];
}
