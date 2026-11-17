// Which host a draft belongs to, and which host its agent runs on. Pure.
//
// A draft lives in the scope of the host whose project it is opened in: This
// Mac for every project This Mac has, a paired host for one only that host
// has. Where its agent runs is a detail of the workspace, picked on the
// new-workspace row (the run-target picker); without a pick it is the draft's
// own host.

import type { ProjectRef } from "@/api";
import type { DraftAgent } from "./drafts";
import { type EnvironmentEntry, type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "./environments";

/** The host `draft`'s agent runs on: the one picked for it while that host is
 *  still paired, else the draft's own (`hostId`). A roadmap hand-off stays on
 *  its own host, where its item is linked to the agent it spawns. */
export function draftRunsOn(
  draft: Pick<DraftAgent, "runsOn" | "roadmapItemId">,
  hostId: EnvironmentId,
  environments: Record<EnvironmentId, EnvironmentEntry>,
): EnvironmentId {
  const picked = draft.roadmapItemId ? undefined : draft.runsOn;
  return picked && environments[picked] ? picked : hostId;
}

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
