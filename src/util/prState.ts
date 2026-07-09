// PR state with a database fallback. Live state (the `prStates` store map,
// fed by polls and pr:state_changed events) always wins; when it's absent or
// null — fresh app start, GitHub unreachable, broken checkout — the last
// snapshot the backend persisted on the repo record fills in, so a PR GitHub
// already confirmed (especially a merged one) never renders as "no PR".

import { useMemo } from "react";
import type { PrState, PrStatus, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";

const PR_STATUSES: readonly PrStatus[] = ["open", "merged", "closed"];

/** Rebuild the last persisted PR state from a repo record's snapshot columns.
 *  Null when no PR is bound or no fetch has ever succeeded. `mergeable` isn't
 *  persisted and reads false — it only means anything while polling live. */
export function prSnapshot(repo: TrackedRepo | undefined): PrState | null {
  if (!repo || repo.pr_number == null || !repo.pr_state) return null;
  if (!(PR_STATUSES as readonly string[]).includes(repo.pr_state)) return null;
  return {
    number: repo.pr_number,
    url: repo.pr_url ?? "",
    state: repo.pr_state as PrStatus,
    title: repo.pr_title ?? "",
    mergeable: false,
  };
}

/** The PR state to render for an agent: live store value, else the database
 *  snapshot from the agent's primary repo. */
export function usePrState(agentId: string): PrState | null {
  const live = useAppStore((s) => s.prStates[agentId] ?? null);
  const repo = useAppStore((s) => s.workspace?.agents.find((a) => a.id === agentId)?.repos[0]);
  return useMemo(() => live ?? prSnapshot(repo), [live, repo]);
}
