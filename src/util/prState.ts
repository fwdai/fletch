// PR state with a database fallback. Live state (the `prStates` store map,
// fed by polls and pr:state_changed events) always wins; when it's absent or
// null — fresh app start, GitHub unreachable, broken checkout — the last
// snapshot the backend persisted on the repo record fills in, so a PR GitHub
// already confirmed (especially a merged one) never renders as "no PR".

import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";
import type { AgentRecord, PrSetEntry, PrState, PrStatus, TrackedRepo } from "@/api";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { checkoutPrs } from "./prSummary";

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
 *  snapshot from the agent's primary repo. */
export function usePrState(agentId: string): PrState | null {
  const live = useAppStore((s) => s.prStates[agentId] ?? null);
  const found = useAppStore((s) => s.workspace?.agents.find((a) => a.id === agentId)?.repos[0]);
  return useMemo(() => live ?? prSnapshot(found), [live, found]);
}

/** One PR within an agent's set, with its CI rollup (null until the app-wide
 *  checks poll lands or when there's no rollup) and the repo it lives in. */
export interface AgentPr extends PrSetEntry {
  repo: TrackedRepo;
}

/** Every PR across an agent's repos: repo order (primary first), each
 *  checkout's focused PR first, then the rest of its set (see `checkoutPrs`).
 *  A checkout holds a set — sub-agents each open their own — read from
 *  `prSets`. Each repo reads its own keys — plain agent id for
 *  the primary, the suffixed `checkoutKey` for secondaries. With no set known
 *  (an older host) the checkout's focused PR stands alone, resolved with the
 *  panel's per-repo policy: a present key, even a confirmed `null` (a fetch
 *  that found no PR), is authoritative; only a never-fetched key falls back to
 *  that repo's own persisted snapshot (a secondary never inherits the
 *  primary's). Repos without a PR drop out.
 *
 *  Runs in every mounted sidebar row on every store update, so the selectors
 *  only pick per-key references (shallow-compared) and the work happens in the
 *  memo, once per actual change. */
export function useAgentPrs(agent: AgentRecord): AgentPr[] {
  const keys = agent.repos.map((r, i) => checkoutKey(agent.id, i === 0 ? undefined : r.subdir));
  const live = useAppStore(
    useShallow((s) => keys.map((k) => (k in s.prStates ? s.prStates[k] : undefined))),
  );
  const checks = useAppStore(useShallow((s) => keys.map((k) => s.prChecks[k] ?? null)));
  const sets = useAppStore(useShallow((s) => keys.map((k) => s.prSets[k])));
  return useMemo(
    () =>
      agent.repos.flatMap((repo, i) => {
        const focused = live[i] !== undefined ? live[i] : prSnapshot(repo);
        return checkoutPrs(sets[i], focused, checks[i]).map((e) => ({ ...e, repo }));
      }),
    [agent.repos, live, checks, sets],
  );
}
