// The sidebar's project focus, as the commands that set it from outside the
// sidebar see it: the palette's rows and the ⌘⇧P chord. The focus itself is
// `projectFocus` on the store; what it means is in
// `components/Sidebar/projectFocus.ts`.

import { groupProjectRefs, projectKey } from "@/util/projects";
import { surfaceRepoPath } from "./surface";
import type { AppState } from "./types";

/** The sidebar group key of the project in front (see `surfaceRepoPath`): its
 *  project id, or the repo path itself for a repo that isn't pinned — the two
 *  keys the sidebar groups by. */
export function surfaceProjectKey(s: AppState): string | undefined {
  const repo = surfaceRepoPath(s);
  if (!repo) return undefined;
  const ref = s.workspace?.projects.find((p) => p.path === repo);
  return ref ? projectKey(ref) : repo;
}

/** How many projects there are to choose between. Under two the sidebar shows
 *  no selector, and a focus set anyway would hide the second project the
 *  moment it is added. */
export function projectCount(s: AppState): number {
  return groupProjectRefs(s.workspace?.projects ?? []).length;
}

/** How many of the current projects the focus names; zero is "All projects".
 *  Saved keys of projects that are gone don't count (the sidebar ignores them
 *  the same way). */
export function focusedProjectCount(s: AppState): number {
  const keys = new Set((s.workspace?.projects ?? []).map(projectKey));
  return s.projectFocus.filter((k) => keys.has(k)).length;
}

/** Whether the sidebar is narrowed to exactly the project `key`. */
export function isOnlyFocus(s: AppState, key: string): boolean {
  return s.projectFocus.length === 1 && s.projectFocus[0] === key;
}
