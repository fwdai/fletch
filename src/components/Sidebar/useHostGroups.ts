import { useMemo } from "react";
import type { WfRun } from "@/api";
import { useEveryHost } from "@/store";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { groupByProject, type HostSnapshot, type ProjectGroupData } from "./hostGroups";

/** The sidebar's groups over every host (`groupByProject`), in list order:
 *  This Mac's in the manual order (`sortPaths`, keyed by each project's
 *  primary repo path, so orders saved before multi-repo grouping keep working
 *  unchanged), then the projects only a paired host has. */
export function useHostGroups(
  runsByHost: Readonly<Record<EnvironmentId, readonly WfRun[]>>,
  sortPaths: (paths: string[]) => string[],
): ProjectGroupData[] {
  const hostIds = useEveryHost((_s, hostId) => hostId);
  const workspaces = useEveryHost((s) => s.workspace);
  const drafts = useEveryHost((s) => s.drafts);

  return useMemo(() => {
    const [local, ...hosts] = hostIds.map(
      (hostId, i): HostSnapshot => ({
        hostId,
        workspace: workspaces[i],
        drafts: drafts[i],
        runs: runsByHost[hostId] ?? [],
      }),
    );
    const built = groupByProject(local, hosts);
    const mine = built.filter((g) => g.hostId === LOCAL_ENVIRONMENT_ID);
    const byPath = new Map(mine.map((g) => [g.primaryPath, g]));
    const ordered = sortPaths(mine.map((g) => g.primaryPath))
      .map((p) => byPath.get(p))
      .filter((g): g is ProjectGroupData => g !== undefined);
    return [...ordered, ...built.filter((g) => g.hostId !== LOCAL_ENVIRONMENT_ID)];
  }, [hostIds, workspaces, drafts, runsByHost, sortPaths]);
}
