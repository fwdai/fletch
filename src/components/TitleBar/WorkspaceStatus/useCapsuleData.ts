import { useMemo } from "react";
import { useAppStore } from "@/store";
import { usePrState } from "@/util/prState";
import { checkoutPrs } from "@/util/prSummary";

/** The title-bar capsule's view of the active agent's git/PR state.
 *
 *  A pure read: `useGitSync` keeps all of this current. Checks are surfaced only
 *  while the PR is open, since that's the only state the chip renders. `prs` is
 *  the primary checkout's whole PR set, focused first (the focused PR alone
 *  when no set is known). */
export function useCapsuleData(agentId: string) {
  const shortstats = useAppStore((s) => s.gitShortstats[agentId] ?? null);
  const gitState = useAppStore((s) => s.gitStates[agentId] ?? null);
  const prState = usePrState(agentId);
  const checks = useAppStore((s) => s.prChecks[agentId] ?? null);
  const prSet = useAppStore((s) => s.prSets[agentId]);
  const prOpen = prState?.state === "open";
  const focusedChecks = prOpen ? checks : null;

  const prs = useMemo(
    () => checkoutPrs(prSet, prState, focusedChecks),
    [prSet, prState, focusedChecks],
  );

  return { shortstats, gitState, prState, checks: focusedChecks, prs };
}
