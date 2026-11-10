// The sidebar's project focus, as the commands that set it from outside the
// sidebar see it: the palette's rows and the ⌘⇧P chord. The focus itself is
// `projectFocus` on the store; what it means is in
// `components/Sidebar/projectFocus.ts`.

import { projectKey, projectKeyResolver } from "@/util/projects";
import { activeSurface, surfaceRepoPath } from "./surface";
import type { AppState } from "./types";

/** The sidebar group key of the project in front, resolved the way the
 *  sidebar groups work (`projectKeyResolver`); with nothing of a project in
 *  front, of the one `surfaceRepoPath` falls back to.
 *
 *  Undefined while a workflow run is in front: runs are not in the store, so
 *  which project the run belongs to can't be told from here, and the fallback
 *  would name a project that isn't the one on screen. */
export function surfaceProjectKey(s: AppState): string | undefined {
  const surface = activeSurface(s);
  if (surface.kind === "run") return undefined;
  const keyOf = projectKeyResolver(s.workspace?.projects ?? []);
  if (surface.kind === "agent") {
    return keyOf(surface.agent.project_id, surface.agent.repos[0]?.repo_path);
  }
  return keyOf(undefined, surfaceRepoPath(s));
}

/** Every key the sidebar groups by: the pinned projects, plus any repo that is
 *  no longer pinned but still has a live agent in it. (A workflow run keeps
 *  such a group alive too; runs are not in the store, so a repo only a run
 *  holds open is not counted here.) */
export function projectKeys(s: AppState): string[] {
  const refs = s.workspace?.projects ?? [];
  const keyOf = projectKeyResolver(refs);
  const keys = new Set(refs.map(projectKey));
  for (const a of s.workspace?.agents ?? []) {
    if (a.archive) continue;
    const key = keyOf(a.project_id, a.repos[0]?.repo_path);
    if (key !== undefined) keys.add(key);
  }
  return [...keys];
}

/** How many projects there are to choose between. Under two the sidebar shows
 *  no selector, and a focus set anyway would hide the second project the
 *  moment it is added. */
export function projectCount(s: AppState): number {
  return projectKeys(s).length;
}

/** How many of the current projects the focus names; zero is "All projects".
 *  Saved keys of projects that are gone don't count (the sidebar ignores them
 *  the same way). */
export function focusedProjectCount(s: AppState): number {
  const keys = new Set(projectKeys(s));
  return s.projectFocus.filter((k) => keys.has(k)).length;
}

/** Whether the sidebar is narrowed to exactly the project `key`. */
export function isOnlyFocus(s: AppState, key: string): boolean {
  return s.projectFocus.length === 1 && s.projectFocus[0] === key;
}
