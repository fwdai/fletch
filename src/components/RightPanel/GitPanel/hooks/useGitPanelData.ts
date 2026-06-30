import { useCallback } from "react";
import type { MergeState } from "@/api";
import { deriveState } from "@/components/RightPanel/primaryActions";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";

/** All the live git/PR reads the panel renders from, plus the polling that
 *  keeps them fresh while the panel is mounted:
 *  - git state at 1s,
 *  - PR state at 5s (so a merge/close/mergeable flip on GitHub shows up),
 *  - the heavier checks + review comments at 5s, only while a PR is open.
 *  usePoll fires immediately, so the first read of each still lands on mount. */
export function useGitPanelData(agentId: string) {
  const gitState = useAppStore((s) => s.gitStates[agentId] ?? null);
  const prState = useAppStore((s) => s.prStates[agentId] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrState = useAppStore((s) => s.fetchPrState);
  const prChecksEntry = useAppStore((s) => s.prChecks[agentId]);
  const fetchPrChecks = useAppStore((s) => s.fetchPrChecks);
  const prCommentsEntry = useAppStore((s) => s.prComments[agentId]);
  const fetchPrComments = useAppStore((s) => s.fetchPrComments);

  const pollGitState = useCallback(() => fetchGitState(agentId), [agentId, fetchGitState]);
  usePoll(pollGitState, 1000, [pollGitState]);

  const pollPrState = useCallback(() => fetchPrState(agentId), [agentId, fetchPrState]);
  usePoll(pollPrState, 5000, [pollPrState]);

  const prOpen = prState?.state === "open";
  const pollChecks = useCallback(async () => {
    if (!prOpen) return;
    // Checks + review comments share the slow cadence: both are heavier gh
    // reads that only matter while a PR is open.
    await Promise.all([fetchPrChecks(agentId), fetchPrComments(agentId)]);
  }, [agentId, prOpen, fetchPrChecks, fetchPrComments]);
  usePoll(pollChecks, 5000, [pollChecks]);

  const checks = prChecksEntry ?? null;
  const comments = prCommentsEntry ?? null;
  // An absent entry (undefined) means the first fetch hasn't landed → render
  // the "checking…" sub-state; null means confirmed unavailable → fall back to
  // mergeable-only behavior. Keep the raw `prChecksEntry === undefined` test.
  const mergeState: MergeState | null =
    checks?.merge_state ?? (prOpen && prChecksEntry === undefined ? "unknown" : null);

  const panelState = deriveState(gitState, prState);

  return {
    gitState,
    prState,
    checks,
    comments,
    mergeState,
    prOpen,
    panelState,
    fetchGitState,
    fetchPrState,
    fetchPrChecks,
  };
}
