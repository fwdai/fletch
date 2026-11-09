import { useCallback, useMemo, useRef, useState } from "react";
import { api, type DiffBaseMode, type FileStatus } from "@/api";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";

export interface ChangedFiles {
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** The scope's own list could not be fetched, so `files` is the uncommitted
   *  list the git poll already holds — exact for "head", a subset for "fork". */
  degraded: boolean;
  /** False until the first poll for this agent and scope has answered. */
  loaded: boolean;
}

/** One poll: the scope's list from the backend or, when the call fails (a host
 *  without the command, a checkout still being provisioned), the uncommitted
 *  list the git poll already holds. */
export async function fetchChangedFiles(
  agentId: string,
  base: DiffBaseMode,
): Promise<{ files: FileStatus[]; degraded: boolean }> {
  try {
    return { files: await api.listChangedFiles(agentId, base), degraded: false };
  } catch {
    return { files: useAppStore.getState().gitStates[agentId]?.files ?? [], degraded: true };
  }
}

const sigOf = (files: FileStatus[]) =>
  files.map((f) => `${f.path}\n${f.kind}\n${f.additions}\n${f.deletions}`).join("\n");

/** Every file the agent's checkout changed in `base`'s scope, polled every 1s
 *  while the agent works and 2s otherwise, for as long as the caller is
 *  mounted. The list keeps its identity while nothing in it changes, so
 *  consumers can depend on it. */
export function useChangedFiles(agentId: string, base: DiffBaseMode, busy: boolean): ChangedFiles {
  const scope = `${agentId}\n${base}`;
  const [state, setState] = useState<{
    scope: string;
    files: FileStatus[];
    sig: string;
    degraded: boolean;
  }>();
  // The scope on screen, so a poll for one the user has since left is dropped
  // rather than replacing the newer scope's list.
  const shown = useRef(scope);
  shown.current = scope;

  const tick = useCallback(async () => {
    const next = await fetchChangedFiles(agentId, base);
    if (shown.current !== scope) return;
    const sig = sigOf(next.files);
    setState((prev) =>
      prev?.scope === scope && prev.sig === sig && prev.degraded === next.degraded
        ? prev
        : { scope, sig, ...next },
    );
  }, [agentId, base, scope]);
  usePoll(tick, busy ? 1000 : 2000, [tick]);

  return useMemo(() => {
    const current = state?.scope === scope ? state : null;
    const files = current?.files ?? [];
    return {
      files,
      additions: files.reduce((n, f) => n + f.additions, 0),
      deletions: files.reduce((n, f) => n + f.deletions, 0),
      degraded: current?.degraded ?? false,
      loaded: current != null,
    };
  }, [state, scope]);
}
