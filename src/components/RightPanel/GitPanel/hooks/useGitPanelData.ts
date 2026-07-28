import { useCallback, useMemo } from "react";
import type { MergeState, TrackedRepo } from "@/api";
import { deriveState } from "@/components/RightPanel/primaryActions";
import { useAppStore } from "@/store";
import { gitKey } from "@/store/git";
import { prSnapshot } from "@/util/prState";

/** The Git panel's view of one repo's git/PR state, derived from the store.
 *
 *  A pure read — `useGitSync` owns the polling that keeps these current. The
 *  fetchers returned at the bottom are for *actions*, not refresh loops: after a
 *  push or a delegated git op the caller wants the new state now rather than at
 *  the next tick.
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

  // Subdir-bound fetchers for *actions* — a push, a merge, a delegated git op —
  // so callers refresh this section's repo without re-threading the scope.
  // Background freshness is `useGitSync`'s job, not theirs.
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
  const prOpen = prState?.state === "open";

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
