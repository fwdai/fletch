// The agent's git state as one answer across every checkout it has, from the
// store's git poll (`store/gitSync` keeps the focused agent's checkouts a
// second fresh while the right rail shows). The Code panel is about the
// agent's work as a whole, so nothing in it may read the primary checkout
// alone: a secondary repo's commits are changes too, and a checkout the host
// refuses to read is not a clean one.
import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import type { AgentRecord, FileStatus, GitState } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";

/** One checkout of the agent as the store knows it. Neither field until its
 *  poll has answered; `blocked` when the host refuses to run git there. */
export interface CheckoutGit {
  subdir: string;
  state?: GitState;
  /** Config keys the host refuses to run git over (`git::hardening`). */
  blocked?: string[];
}

export interface AgentGit {
  /** Every checkout has answered, with a state or a refusal. Until then the
   *  rest is unknown, not empty. */
  answered: boolean;
  /** Uncommitted files across the checkouts, each path prefixed with its
   *  checkout's directory for a multi-repo agent (as `list_changed_files`
   *  prefixes them); null until `answered`. */
  files: FileStatus[] | null;
  /** Commits ahead of the base, over every checkout. */
  ahead: number;
  /** The checkouts (their `subdir`s) whose git state the host can't read:
   *  whatever is in them is unknown, and nothing here speaks for them. */
  blocked: string[];
}

export function aggregateGit(checkouts: CheckoutGit[]): AgentGit {
  const answered = checkouts.every((c) => c.state !== undefined || c.blocked !== undefined);
  const multi = checkouts.length > 1;
  const files = !answered
    ? null
    : multi
      ? checkouts.flatMap((c) =>
          (c.state?.files ?? []).map((f) => ({ ...f, path: `${c.subdir}/${f.path}` })),
        )
      : (checkouts[0]?.state?.files ?? []);
  return {
    answered,
    files,
    ahead: checkouts.reduce((n, c) => n + (c.state?.ahead ?? 0), 0),
    blocked: checkouts.filter((c) => c.blocked !== undefined).map((c) => c.subdir),
  };
}

export function useAgentGit(agent: AgentRecord): AgentGit {
  const checkouts = useAppStore(
    useShallow((s) =>
      agent.repos.map((repo, i): CheckoutGit => {
        const key = checkoutKey(agent.id, i === 0 ? undefined : repo.subdir);
        return { subdir: repo.subdir, state: s.gitStates[key], blocked: s.gitBlocked[key] };
      }),
    ),
  );
  return useMemo(() => aggregateGit(checkouts), [checkouts]);
}
