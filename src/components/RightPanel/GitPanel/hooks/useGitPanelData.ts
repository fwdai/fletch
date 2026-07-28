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
 *  - PR state + CI at 5s via `fetchPrLive` — one backend pass over
 *    ETag-conditional REST, which GitHub doesn't bill when nothing changed, so
 *    the tick that users actually watch ("is it green", "did it merge") stays
 *    tight and near-free,
 *  - review threads at 30s while a PR is open. These stay on GraphQL (thread
 *    resolution has no REST equivalent) and so cost points per call — human
 *    review comments arrive on minute timescales, so the slower tick loses
 *    nothing.
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
  // snapshot fills in only when live state was never fetched (absent key).
  // Keep `undefined` distinct from `null` here — see the fallback below.
  const livePr = useAppStore((s) => s.prStates[key]);
  // A scoped (secondary) section must NEVER fall back to the primary repo for
  // its snapshot — that would leak the primary's persisted PR number/title
  // into this repo's card until the scoped fetch lands. Its snapshot comes
  // from its own TrackedRepo or nowhere.
  const snapshotRepo = useAppStore((s) =>
    subdir === undefined
      ? (repo ?? s.workspace?.agents.find((a) => a.id === agentId)?.repos[0])
      : repo,
  );
  // A present key — including a confirmed `null` (fetch returned no PR) — is
  // authoritative and wins. Only an absent key (undefined = never fetched)
  // falls back to the persisted snapshot, so a scoped fetch that confirmed no
  // PR clears the card instead of rendering this repo's stale snapshot.
  const prState = useMemo(
    () => (livePr !== undefined ? livePr : prSnapshot(snapshotRepo)),
    [livePr, snapshotRepo],
  );

  const fetchGitStateStore = useAppStore((s) => s.fetchGitState);
  const fetchPrStateStore = useAppStore((s) => s.fetchPrState);
  const prChecksEntry = useAppStore((s) => s.prChecks[key]);
  const fetchPrChecksStore = useAppStore((s) => s.fetchPrChecks);
  const prCommentsEntry = useAppStore((s) => s.prComments[key]);
  const fetchPrLiveStore = useAppStore((s) => s.fetchPrLive);
  const fetchPrThreadsStore = useAppStore((s) => s.fetchPrThreads);

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
  const fetchPrLive = useCallback(
    (id: string) => fetchPrLiveStore(id, subdir),
    [fetchPrLiveStore, subdir],
  );
  const fetchPrThreads = useCallback(
    (id: string) => fetchPrThreadsStore(id, subdir),
    [fetchPrThreadsStore, subdir],
  );

  const pollGitState = useCallback(() => fetchGitState(agentId), [agentId, fetchGitState]);
  usePoll(pollGitState, 1000, [pollGitState]);

  // State and CI in one conditional-REST pass. Runs regardless of PR status —
  // it *is* how the panel learns a PR merged or closed — and stays at 5s even
  // once checks settle, because an unchanged read is a 304 that GitHub doesn't
  // bill. There's nothing to back off from.
  const pollLive = useCallback(() => fetchPrLive(agentId), [agentId, fetchPrLive]);
  usePoll(pollLive, 5000, [pollLive]);

  const prOpen = prState?.state === "open";
  const pollThreads = useCallback(async () => {
    if (!prOpen) return;
    await fetchPrThreads(agentId);
  }, [agentId, prOpen, fetchPrThreads]);
  // 30s: this is the one panel read that still spends GraphQL points, and human
  // review comments don't arrive faster than that.
  usePoll(pollThreads, 30000, [pollThreads]);

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
