// PR state with a database fallback. Live state (the `prStates` store map,
// fed by polls and pr:state_changed events) always wins; when it's absent or
// null — fresh app start, GitHub unreachable, broken checkout — the last
// snapshot the backend persisted on the repo record fills in, so a PR GitHub
// already confirmed (especially a merged one) never renders as "no PR".

import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import type { AgentRecord, PrChecks, PrState, PrStatus, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { gitKey } from "@/store/git";

const PR_STATUSES: readonly PrStatus[] = ["open", "merged", "closed"];

/** Rebuild the last persisted PR state from a repo record's snapshot columns.
 *  Null when no PR is bound or no fetch has ever succeeded. `mergeable` isn't
 *  persisted and reads `"unknown"` — a snapshot carries no merge verdict, and
 *  `"unknown"` (not `"conflicting"`) is the honest stand-in so the panel says
 *  "checking…" rather than a false conflict. It only means anything live. */
export function prSnapshot(repo: TrackedRepo | undefined): PrState | null {
  if (!repo || repo.pr_number == null || !repo.pr_state) return null;
  if (!(PR_STATUSES as readonly string[]).includes(repo.pr_state)) return null;
  return {
    number: repo.pr_number,
    url: repo.pr_url ?? "",
    state: repo.pr_state as PrStatus,
    title: repo.pr_title ?? "",
    mergeable: "unknown",
  };
}

/** The PR state to render for an agent: live store value, else the database
 *  snapshot from the agent's primary repo.
 *
 *  Callers that already hold the agent record (the sidebar mounts one row per
 *  agent) should pass its primary repo — the selector then skips the O(n)
 *  agents scan that would otherwise run in every mounted row on every store
 *  update. Singleton consumers (title-bar capsule, Git panel) can omit it and
 *  let the selector look the repo up. */
export function usePrState(agentId: string, repo?: TrackedRepo): PrState | null {
  const live = useAppStore((s) => s.prStates[agentId] ?? null);
  const found = useAppStore(
    (s) => repo ?? s.workspace?.agents.find((a) => a.id === agentId)?.repos[0],
  );
  return useMemo(() => live ?? prSnapshot(found), [live, found]);
}

/** One repo's PR within an agent's set, with its CI rollup (null until the
 *  app-wide checks poll lands or when there's no rollup). */
export interface AgentPr {
  pr: PrState;
  checks: PrChecks | null;
  repo: TrackedRepo;
}

/** Every PR across an agent's repos, in repo order (primary first). Each repo
 *  reads its own store entry — plain agent id for the primary, the suffixed
 *  `gitKey` for secondaries, both fed by the app-wide bulk polls — resolved
 *  with the same per-repo policy as the panel sections and the PR-set strip:
 *  a present key, even a confirmed `null` (a fetch that found no PR), is
 *  authoritative; only a never-fetched key (absent = `undefined`) falls back
 *  to that repo's own persisted snapshot (a secondary never inherits the
 *  primary's). Repos without a PR drop out. */
export function useAgentPrs(agent: AgentRecord): AgentPr[] {
  const keys = agent.repos.map((r, i) => gitKey(agent.id, i === 0 ? undefined : r.subdir));
  const live = useAppStore(
    useShallow((s) => keys.map((k) => (k in s.prStates ? s.prStates[k] : undefined))),
  );
  const checks = useAppStore(useShallow((s) => keys.map((k) => s.prChecks[k] ?? null)));
  return useMemo(
    () =>
      agent.repos.flatMap((repo, i) => {
        const pr = live[i] !== undefined ? live[i] : prSnapshot(repo);
        return pr ? [{ pr, checks: checks[i], repo }] : [];
      }),
    [agent.repos, live, checks],
  );
}
