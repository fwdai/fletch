import { useAppStore } from "@/store";
import { usePrState } from "@/util/prState";

/** The title-bar capsule's view of the active agent's git/PR state.
 *
 *  A pure read: `useGitSync` keeps all of this current. Checks are surfaced only
 *  while the PR is open, since that's the only state the chip renders. */
export function useCapsuleData(agentId: string) {
  const shortstats = useAppStore((s) => s.gitShortstats[agentId] ?? null);
  const gitState = useAppStore((s) => s.gitStates[agentId] ?? null);
  const prState = usePrState(agentId);
  const checks = useAppStore((s) => s.prChecks[agentId] ?? null);
  const prOpen = prState?.state === "open";

  return { shortstats, gitState, prState, checks: prOpen ? checks : null };
}
