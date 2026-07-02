import { useCallback } from "react";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";

/** Live git/PR reads for the title-bar capsule of the active agent.
 *
 *  The always-visible badge already rides the app-wide polls (`gitShortstats`
 *  at 5s, `prStates` at 45s). This hook adds the richer reads the popover and
 *  checks-chip need — full git state and the CI rollup — which otherwise only
 *  refresh while the Git panel is mounted. Kept gentle (10s): the title bar is
 *  a glance, not the panel. Fires immediately on mount, so the first read still
 *  lands right away; checks only fetch while a PR is open. */
export function useCapsuleData(agentId: string) {
  const shortstats = useAppStore((s) => s.gitShortstats[agentId] ?? null);
  const gitState = useAppStore((s) => s.gitStates[agentId] ?? null);
  const prState = useAppStore((s) => s.prStates[agentId] ?? null);
  const checks = useAppStore((s) => s.prChecks[agentId] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrChecks = useAppStore((s) => s.fetchPrChecks);

  const prOpen = prState?.state === "open";
  const poll = useCallback(async () => {
    await fetchGitState(agentId);
    if (prOpen) await fetchPrChecks(agentId);
  }, [agentId, prOpen, fetchGitState, fetchPrChecks]);
  usePoll(poll, 10000, [poll]);

  return { shortstats, gitState, prState, checks: prOpen ? checks : null };
}
