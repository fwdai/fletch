import { useCallback, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { type AgentRecord, api, type DiffBaseMode, type FileStatus, type TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { fileRevision } from "@/util/diff";
import { usePoll } from "@/util/hooks";

export interface ChangedFiles {
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** The fork-point list could not be fetched, so `files` is the uncommitted
   *  list — a subset of what was asked for. */
  degraded: boolean;
  /** False until the scope's list is known for every checkout of the agent;
   *  an unanswered checkout must not read as a clean one. */
  loaded: boolean;
}

const NONE: FileStatus[] = [];

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
    const fromStore = { files: uncommitted ?? NONE, loaded: uncommitted !== null };
    const picked =
      base === "head"
        ? { ...fromStore, degraded: false }
        : sinceFork === null
          ? { files: NONE, degraded: false, loaded: false }
          : sinceFork.failed
            ? { ...fromStore, degraded: true }
            : { files: sinceFork.files, degraded: false, loaded: true };
    return {
      ...picked,
      additions: picked.files.reduce((n, f) => n + f.additions, 0),
      deletions: picked.files.reduce((n, f) => n + f.deletions, 0),
    };
  }, [base, uncommitted, sinceFork]);
}

/** One checkout's uncommitted files as the store knows them: undefined until
 *  its git poll has answered. */
export type CheckoutFiles = { files: FileStatus[] } | undefined;

/** The uncommitted files of an agent's checkouts as one list, from each
 *  checkout's files (`states` aligned with `repos`). Null while any checkout
 *  is still unanswered: a list missing a checkout would pass for the whole. A
 *  multi-repo agent's paths are prefixed with their checkout's directory, as
 *  `list_changed_files` prefixes them. */
export function uncommittedFiles(
  repos: TrackedRepo[],
  states: CheckoutFiles[],
): FileStatus[] | null {
  if (repos.some((_, i) => states[i] === undefined)) return null;
  if (repos.length <= 1) return states[0]?.files ?? NONE;
  return repos.flatMap((repo, i) =>
    (states[i]?.files ?? NONE).map((f) => ({ ...f, path: `${repo.subdir}/${f.path}` })),
  );
}

/** A checkout whose git state the host refuses to read (planted config) has
 *  answered, with nothing: the Git panel shows what to do about it. */
const BLOCKED: CheckoutFiles = { files: NONE };

function useUncommittedFiles(agent: AgentRecord): FileStatus[] | null {
  const states = useAppStore(
    useShallow((s) =>
      agent.repos.map((repo, i): CheckoutFiles => {
        const key = checkoutKey(agent.id, i === 0 ? undefined : repo.subdir);
        return s.gitStates[key] ?? (s.gitBlocked[key] ? BLOCKED : undefined);
      }),
    ),
  );
  return useStableList(useMemo(() => uncommittedFiles(agent.repos, states), [agent.repos, states]));
}

const sigOf = (files: FileStatus[]) =>
  files.map((f) => `${f.path}\n${f.kind}\n${fileRevision(f)}`).join("\n");

/** `files` by value: the array handed out before, while a new one holds the
 *  same files at the same revisions. */
function useStableList(files: FileStatus[] | null): FileStatus[] | null {
  const kept = useRef({ seen: files, out: files });
  if (kept.current.seen !== files) {
    const same =
      kept.current.out !== null && files !== null && sigOf(kept.current.out) === sigOf(files);
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
      (): ForkFiles => ({ files: NONE, failed: true }),
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
