import { useCallback, useMemo } from "react";
import type { MergeState, TrackedRepo } from "@/api";
import { deriveState } from "@/components/RightPanel/primaryActions";
import { useAppStore } from "@/store";
import { gitKey } from "@/store/git";
import { usePoll } from "@/util/hooks";
import { prSnapshot } from "@/util/prState";

/** All the live git/PR reads the panel renders from, plus the polling that
 *  keeps them fresh while the panel is mounted:
 *  - git state at 1s,
 *  - PR state at 5s (so a merge/close/mergeable flip on GitHub shows up),
 *  - the heavier checks + review comments at 5s, only while a PR is open.
 *  usePoll fires immediately, so the first read of each still lands on mount.
 *
 *  `repo`/`subdir` scope the hook to one repo of a multi-repo agent: `subdir`
 *  undefined = the primary repo, read/written under the plain agent key (the
 *  one live events and bulk polls update); a secondary repo reads/writes under
 *  `gitKey(agentId, subdir)`. The returned fetchers are pre-bound to the
 *  scope's subdir, so callers keep passing just the agent id. */
export function useGitPanelData(agentId: string, repo?: TrackedRepo, subdir?: string) {
  const key = gitKey(agentId, subdir);
  const gitState = useAppStore((s) => s.gitStates[key] ?? null);
  // PR state with the database-snapshot fallback (same policy as usePrState,
  // scoped to this section's repo): live store value wins; the last persisted
  // snapshot fills in when live state is absent or null.
  const livePr = useAppStore((s) => s.prStates[key] ?? null);
  const snapshotRepo = useAppStore(
    (s) => repo ?? s.workspace?.agents.find((a) => a.id === agentId)?.repos[0],
  );
  const prState = useMemo(() => livePr ?? prSnapshot(snapshotRepo), [livePr, snapshotRepo]);

  const fetchGitStateStore = useAppStore((s) => s.fetchGitState);
  const fetchPrStateStore = useAppStore((s) => s.fetchPrState);
  const prChecksEntry = useAppStore((s) => s.prChecks[key]);
  const fetchPrChecksStore = useAppStore((s) => s.fetchPrChecks);
  const prCommentsEntry = useAppStore((s) => s.prComments[key]);
  const fetchPrCommentsStore = useAppStore((s) => s.fetchPrComments);

  // Subdir-bound fetchers, so every consumer (polls, actions, delegation
  // refresh) hits this section's repo without re-threading the scope.
  const fetchGitState = useCallback(
    (id: string) => fetchGitStateStore(id, subdir),
    [fetchGitStateStore, subdir],
  );
  const fetchPrState = useCallback(
    (id: string) => fetchPrStateStore(id, subdir),
    [fetchPrStateStore, subdir],
  );
  const fetchPrChecks = useCallback(
    (id: string) => fetchPrChecksStore(id, subdir),
    [fetchPrChecksStore, subdir],
  );
  const fetchPrComments = useCallback(
    (id: string) => fetchPrCommentsStore(id, subdir),
    [fetchPrCommentsStore, subdir],
  );

  const pollGitState = useCallback(() => fetchGitState(agentId), [agentId, fetchGitState]);
  usePoll(pollGitState, 1000, [pollGitState]);

  const pollPrState = useCallback(() => fetchPrState(agentId), [agentId, fetchPrState]);
  usePoll(pollPrState, 5000, [pollPrState]);

  const prOpen = prState?.state === "open";
  const pollChecks = useCallback(async () => {
    if (!prOpen) return;
    // Checks + review comments share a cadence: both are heavier gh reads that
    // only matter while a PR is open.
    await Promise.all([fetchPrChecks(agentId), fetchPrComments(agentId)]);
  }, [agentId, prOpen, fetchPrChecks, fetchPrComments]);
  // Adaptive: 5s while checks are still in flight (or not yet fetched), backing
  // off to 15s once they've settled pass/fail — a settled PR rarely changes, and
  // the app-wide poll still covers it. `undefined` (first fetch pending) counts
  // as in-flight so the initial read lands promptly.
  const checksSettled = prOpen && prChecksEntry != null && prChecksEntry.rollup !== "pending";
  usePoll(pollChecks, checksSettled ? 15000 : 5000, [pollChecks, checksSettled]);

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
