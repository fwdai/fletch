import { useCallback, useMemo, useRef, useState } from "react";
import { api, type DiffBaseMode, type FileStatus } from "@/api";
import { fileRevision } from "@/util/diff";
import { usePoll } from "@/util/hooks";
import type { AgentGit } from "../useAgentGit";

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
  /** Checkouts (`subdir`s) whose git state the host can't read: `files` says
   *  nothing about them, in either scope. */
  blocked: string[];
}

const NONE: FileStatus[] = [];

/** Every file the agent's checkouts changed in `base`'s scope. Under
 *  Uncommitted that is `git`'s list, which the store's poll already keeps a
 *  second fresh while the right rail shows; since the fork point it is a poll
 *  of its own, every 1s while the agent works and 2s otherwise, which falls
 *  back to the uncommitted list when the host can't answer. The list keeps its
 *  identity while nothing in it changes, so consumers can depend on it. */
export function useChangedFiles(
  agentId: string,
  git: AgentGit,
  base: DiffBaseMode,
  busy: boolean,
): ChangedFiles {
  const uncommitted = useStableList(git.files);
  const sinceFork = useForkFiles(agentId, base === "fork", busy);

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
      blocked: git.blocked,
      additions: picked.files.reduce((n, f) => n + f.additions, 0),
      deletions: picked.files.reduce((n, f) => n + f.deletions, 0),
    };
  }, [base, uncommitted, sinceFork, git.blocked]);
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
   *  provisioned or refused, a host without the command). */
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
