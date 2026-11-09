import { useCallback, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import {
  type AgentRecord,
  api,
  type DiffBaseMode,
  type FileStatus,
  type GitState,
  type TrackedRepo,
} from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { usePoll } from "@/util/hooks";

export interface ChangedFiles {
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** The fork-point list could not be fetched, so `files` is the uncommitted
   *  list — a subset of what was asked for. */
  degraded: boolean;
  /** False until the scope's list has answered once for this agent. */
  loaded: boolean;
}

/** Every file the agent's checkouts changed in `base`'s scope. Under
 *  Uncommitted that is the list the store's git poll already keeps a second
 *  fresh while the right rail shows (`store/gitSync`); since the fork point it
 *  is a poll of its own, every 1s while the agent works and 2s otherwise, which
 *  falls back to the uncommitted list when the host can't answer. The list
 *  keeps its identity while nothing in it changes, so consumers can depend on
 *  it. */
export function useChangedFiles(
  agent: AgentRecord,
  base: DiffBaseMode,
  busy: boolean,
): ChangedFiles {
  const uncommitted = useUncommittedFiles(agent);
  const sinceFork = useForkFiles(agent.id, base === "fork", busy);

  return useMemo(() => {
    const picked =
      base === "head"
        ? { files: uncommitted, degraded: false, loaded: true }
        : sinceFork === null
          ? { files: [], degraded: false, loaded: false }
          : sinceFork.failed
            ? { files: uncommitted, degraded: true, loaded: true }
            : { files: sinceFork.files, degraded: false, loaded: true };
    return {
      ...picked,
      additions: picked.files.reduce((n, f) => n + f.additions, 0),
      deletions: picked.files.reduce((n, f) => n + f.deletions, 0),
    };
  }, [base, uncommitted, sinceFork]);
}

/** The uncommitted files of an agent's checkouts as one list, from each
 *  checkout's git state (`states` aligned with `repos`; undefined where the
 *  poll hasn't answered). A multi-repo agent's paths are prefixed with their
 *  checkout's directory, as `list_changed_files` prefixes them. */
export function uncommittedFiles(
  repos: TrackedRepo[],
  states: (GitState | undefined)[],
): FileStatus[] {
  if (repos.length <= 1) return states[0]?.files ?? [];
  return repos.flatMap((repo, i) =>
    (states[i]?.files ?? []).map((f) => ({ ...f, path: `${repo.subdir}/${f.path}` })),
  );
}

function useUncommittedFiles(agent: AgentRecord): FileStatus[] {
  const states = useAppStore(
    useShallow((s) =>
      agent.repos.map(
        (repo, i) => s.gitStates[checkoutKey(agent.id, i === 0 ? undefined : repo.subdir)],
      ),
    ),
  );
  return useStableList(useMemo(() => uncommittedFiles(agent.repos, states), [agent.repos, states]));
}

const sigOf = (files: FileStatus[]) =>
  files.map((f) => `${f.path}\n${f.kind}\n${f.additions}\n${f.deletions}`).join("\n");

/** `files` by value: the array handed out before, while a new one holds the
 *  same files with the same counts. */
function useStableList(files: FileStatus[]): FileStatus[] {
  const kept = useRef({ seen: files, out: files });
  if (kept.current.seen !== files) {
    const same = sigOf(kept.current.out) === sigOf(files);
    kept.current = { seen: files, out: same ? kept.current.out : files };
  }
  return kept.current.out;
}

interface ForkFiles {
  files: FileStatus[];
  /** The host couldn't list the scope (no fork point, a checkout still being
   *  provisioned, a host without the command). */
  failed: boolean;
}

/** The files changed since the fork point, polled while `enabled`; null
 *  before the first answer for this agent. */
function useForkFiles(agentId: string, enabled: boolean, busy: boolean): ForkFiles | null {
  const [state, setState] = useState<{ agentId: string; sig: string; result: ForkFiles }>();
  // The agent on screen, so a poll for one the user has since left is dropped
  // rather than replacing the newer agent's list.
  const shown = useRef(agentId);
  shown.current = agentId;

  const tick = useCallback(async () => {
    if (!enabled) return;
    const result = await api.listChangedFiles(agentId, "fork").then(
      (files): ForkFiles => ({ files, failed: false }),
      (): ForkFiles => ({ files: [], failed: true }),
    );
    if (shown.current !== agentId) return;
    const sig = `${result.failed}\n${sigOf(result.files)}`;
    setState((prev) =>
      prev?.agentId === agentId && prev.sig === sig ? prev : { agentId, sig, result },
    );
  }, [agentId, enabled]);
  usePoll(tick, busy ? 1000 : 2000, [tick]);

  return state?.agentId === agentId ? state.result : null;
}
