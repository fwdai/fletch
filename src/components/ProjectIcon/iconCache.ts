import { useEffect, useReducer } from "react";
import type { ProjectIconData } from "@/api/types/agent";

/** Fetches one project's stored icon: the desktop's `api.projectIconGet`, or
 *  the phone's over its own client. */
export type IconLoader = (projectId: string) => Promise<ProjectIconData | null>;

/** Answers by hash, for the life of the app: a hash names its bytes, so an
 *  answer never goes stale, and every placement of a project shares one fetch. */
const resolved = new Map<string, ProjectIconData | null>();
const inFlight = new Map<string, Promise<void>>();

/** The icon behind `iconHash`, fetched once per hash. Null until it lands, on
 *  a failed fetch (the next mount tries again), and with no hash or loader. */
export function useProjectIconData(
  projectId: string | undefined,
  iconHash: string | null | undefined,
  load: IconLoader | undefined,
): ProjectIconData | null {
  const [, rerender] = useReducer((n: number) => n + 1, 0);

  useEffect(() => {
    if (!projectId || !iconHash || !load || resolved.has(iconHash)) return;
    let live = true;
    let fetch = inFlight.get(iconHash);
    if (!fetch) {
      fetch = load(projectId)
        .then(
          (data) => void resolved.set(iconHash, data),
          () => {},
        )
        .finally(() => inFlight.delete(iconHash));
      inFlight.set(iconHash, fetch);
    }
    void fetch.then(() => live && rerender());
    return () => {
      live = false;
    };
  }, [projectId, iconHash, load]);

  return iconHash ? (resolved.get(iconHash) ?? null) : null;
}
