// The sidebar's project focus, as the commands that set it from outside the
// sidebar see it: the palette's rows and the ⌘⇧P chord. The focus itself is
// `projectFocus` on the store; what it means is in
// `components/Sidebar/projectFocus.ts`.
//
// Workflow runs are not in the store, and the sidebar lists them: a run in
// front has a project, and a run in a repo that is no longer pinned keeps
// that repo listed as a project of its own. So every question here takes the
// run list beside the store. The palette has it; the chord asks the host for
// it only when the answer turns on it (`focusNeedsRuns`).

import type { WfRun } from "@/api";
import { projectKey, projectKeyResolver } from "@/util/projects";
import { activeSurface, surfaceRepoPath } from "./surface";
import type { AppState } from "./types";

/** The sidebar group key of the project in front, resolved the way the
 *  sidebar groups work (`projectKeyResolver`); with nothing of a project in
 *  front, of the one `surfaceRepoPath` falls back to. Undefined for a run
 *  that `runs` doesn't hold: the fallback would name a project that isn't
 *  the one on screen. */
export function surfaceProjectKey(s: AppState, runs: readonly WfRun[] = []): string | undefined {
  const surface = activeSurface(s);
  const keyOf = projectKeyResolver(s.workspace?.projects ?? []);
  if (surface.kind === "run") {
    const run = runs.find((r) => r.id === surface.runId);
    return run ? keyOf(run.project_id, run.repo_path) : undefined;
  }
  if (surface.kind === "agent") {
    return keyOf(surface.agent.project_id, surface.agent.repos[0]?.repo_path);
  }
  return keyOf(undefined, surfaceRepoPath(s));
}

/** Every key the sidebar groups by: the pinned projects, plus any repo that is
 *  no longer pinned but still has a live agent or a workflow run in it. */
export function projectKeys(s: AppState, runs: readonly WfRun[] = []): string[] {
  const refs = s.workspace?.projects ?? [];
  const keyOf = projectKeyResolver(refs);
  const keys = new Set(refs.map(projectKey));
  const add = (key: string | undefined) => {
    if (key !== undefined) keys.add(key);
  };
  for (const a of s.workspace?.agents ?? []) {
    if (!a.archive) add(keyOf(a.project_id, a.repos[0]?.repo_path));
  }
  for (const r of runs) add(keyOf(r.project_id, r.repo_path));
  return [...keys];
}

/** How many projects there are to choose between. Under two the sidebar shows
 *  no selector, and a focus set anyway would hide the second project the
 *  moment it is added. */
export function projectCount(s: AppState, runs: readonly WfRun[] = []): number {
  return projectKeys(s, runs).length;
}

/** How many of the current projects the focus names; zero is "All projects".
 *  Saved keys of projects that are gone don't count (the sidebar ignores them
 *  the same way). */
export function focusedProjectCount(s: AppState, runs: readonly WfRun[] = []): number {
  const keys = new Set(projectKeys(s, runs));
  return s.projectFocus.filter((k) => keys.has(k)).length;
}

/** Whether the sidebar is narrowed to exactly the project `key`. */
export function isOnlyFocus(s: AppState, key: string): boolean {
  return s.projectFocus.length === 1 && s.projectFocus[0] === key;
}

/** Whether the store alone can't say what "focus on this project" should do:
 *  a run is in front, or the projects it knows of are too few to choose
 *  between and a run-only one could make the difference. */
export function focusNeedsRuns(s: AppState): boolean {
  return activeSurface(s).kind === "run" || projectCount(s) < 2;
}
