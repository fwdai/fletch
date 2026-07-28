import { useCallback } from "react";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { usePrState } from "@/util/prState";

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
  const prState = usePrState(agentId);
  const checks = useAppStore((s) => s.prChecks[agentId] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrLive = useAppStore((s) => s.fetchPrLive);

  const prOpen = prState?.state === "open";
  const poll = useCallback(async () => {
    await fetchGitState(agentId);
    // Same conditional-REST read the Git panel's fast tick uses: unchanged
    // polls are 304s that GitHub doesn't bill, so the capsule's chip costs
    // nothing to keep current (it previously spent a GraphQL point a tick).
    if (prOpen) await fetchPrLive(agentId);
  }, [agentId, prOpen, fetchGitState, fetchPrLive]);
  usePoll(poll, 10000, [poll]);

  return { shortstats, gitState, prState, checks: prOpen ? checks : null };
}
