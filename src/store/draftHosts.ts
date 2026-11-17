// Which host a new draft opens on. Pure.
//
// A draft lives in the scope of the host whose project it is opened in, and
// runs there: This Mac for every project This Mac has, a paired host for one
// only that host has. The run-target picker on the new-workspace row moves it
// to another host (./moveDraft).

import type { ProjectRef } from "@/api";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "./environments";

/** Where a new draft on `repoPath`, a path on `hostId`, opens: in This Mac's
 *  scope at This Mac's own repo for a project This Mac has, whichever host the
 *  path was found on, else on `hostId` itself. `localProjects` are This Mac's,
 *  `hostProjects` are `hostId`'s. */
export function newDraftHost(
  localProjects: readonly ProjectRef[],
  hostProjects: readonly ProjectRef[],
  hostId: EnvironmentId,
  repoPath: string,
): { hostId: EnvironmentId; repoPath: string } {
  if (hostId === LOCAL_ENVIRONMENT_ID) return { hostId, repoPath };
  const projectId = hostProjects.find((p) => p.path === repoPath)?.project_id;
  // The first ref of a project is its primary repo, where a draft spawns.
  const here = projectId ? localProjects.find((p) => p.project_id === projectId) : undefined;
  return here ? { hostId: LOCAL_ENVIRONMENT_ID, repoPath: here.path } : { hostId, repoPath };
}
