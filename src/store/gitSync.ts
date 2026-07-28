// The app's single owner of git + GitHub polling.
//
// Everything git-shaped is fetched here, written into the store, and read from
// the store by components. No component polls: `useGitPanelData`,
// `useCapsuleData`, `CodeLivePanel` and the sidebar are all pure views over
// `gitStates` / `prStates` / `prChecks` / `prComments`.
//
// It used to be the other way round — each component polled what it rendered —
// which meant four independent pollers for one agent's git state and two for its
// PR, racing each other into the same store keys. `GitPanel`'s `pollDormant`
// existed purely to cover repos no section happened to be rendering.
//
// Two scopes, because they genuinely differ:
//
//   fleet    — every agent, cheap projections for the sidebar.
//   focused  — the selected agent's repos, the detail the panel and title
//              capsule render.
//
// Cadences differ per domain on purpose: 1s is right for a diff the user is
// watching and absurd for a fleet-wide PR sweep. What's centralized is *who
// fetches*, not how often.

import { useCallback } from "react";
import { useShallow } from "zustand/react/shallow";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";

/** Mount once, at the app root. */
export function useGitSync() {
  const fetchAllShortstats = useAppStore((s) => s.fetchAllShortstats);
  const fetchAllGitMeta = useAppStore((s) => s.fetchAllGitMeta);
  const refreshBaseFreshness = useAppStore((s) => s.refreshBaseFreshness);
  const refreshAllPrStatus = useAppStore((s) => s.refreshAllPrStatus);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const fetchPrLive = useAppStore((s) => s.fetchPrLive);
  const fetchPrThreads = useAppStore((s) => s.fetchPrThreads);

  // ── fleet ─────────────────────────────────────────────────────────────────

  // Compact shortstats for every live agent, so sidebar and right-rail badges
  // stay current without a focused panel. Local git; no network.
  usePoll(fetchAllShortstats, 5000, [fetchAllShortstats]);

  // Advisory metadata — base staleness + changed-file paths for overlap hints.
  // Local git too, so it runs regardless of GitHub; slower because a fleet's
  // base moves far less often than its diffs.
  usePoll(fetchAllGitMeta, 15000, [fetchAllGitMeta]);

  // Whether GitHub is reachable is enforced *in the store* — see `githubReady`
  // in `git.ts` — so nothing below re-checks it. It is still listed as a
  // dependency of the two slow polls: that re-arms them (and fires one tick)
  // the moment a connection appears. `github` hydrates asynchronously after
  // launch, so without this a cold start would no-op its first tick and then
  // wait out the 5-minute idle interval before trying again.
  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);

  // Fetches each project's base branch on its source repo so the staleness
  // chips track a base that moved on GitHub. A git fetch, not an API call.
  // Silent by contract — a background fetch never raises a user-facing error.
  usePoll(refreshBaseFreshness, 300000, [refreshBaseFreshness, githubConnected]);

  // Remote PR status for every repo with a known PR: state plus the CI rollup
  // that tints each sidebar pill. One batched query for the whole fleet, 1
  // GraphQL point for up to 50 PRs. 20s while any PR is open, backing off hard
  // once everything has settled — merged PRs answer from the local snapshot, so
  // the slow tick only watches for a rare reopen.
  //
  // Deliberately derived from the store rather than passed in: the cadence
  // reacts to the data it fetches.
  const anyOpenPr = useAppStore((s) => Object.values(s.prStates).some((p) => p?.state === "open"));
  usePoll(refreshAllPrStatus, anyOpenPr ? 20000 : 300000, [refreshAllPrStatus, githubConnected]);

  // ── focused agent ─────────────────────────────────────────────────────────

  // Full git state — branch, ahead/behind, file list. 1s while the right pane is
  // showing it (the user is watching a diff); slower when it's collapsed, where
  // only the title capsule reads it. Local git, but each call forks a process,
  // so the distinction is worth one ternary.
  const panelVisible = useAppStore((s) => !s.rightCollapsed && !s.activeDraftId);
  useFocusedRepoPoll(fetchGitState, panelVisible ? 1000 : 10000);

  // PR state + CI in one backend pass over ETag-conditional REST. Unchanged
  // reads are 304s GitHub doesn't bill, so there's nothing to back off from.
  useFocusedRepoPoll(fetchPrLive, 5000);

  // Unresolved review threads — the one panel read still costing GraphQL points,
  // so it gets the gentlest cadence. Not filtered by PR state here: the backend
  // returns nothing for a repo whose PR isn't open, and keeping that rule in one
  // place beats mirroring it in the poller.
  useFocusedRepoPoll(fetchPrThreads, 30000);
}

/** Run `fetch` for every repo of the focused agent, on `intervalMs`.
 *
 *  Covering *all* of the agent's repos — not just the ones a panel section
 *  happens to render — is what retired `GitPanel`'s `pollDormant`. No-ops when
 *  nothing is selected, so callers need no guard of their own. */
function useFocusedRepoPoll(
  fetch: (agentId: string, subdir?: string) => Promise<void>,
  intervalMs: number,
) {
  const agentId = useAppStore((s) => s.selectedAgentId);
  // Each repo as the `subdir` the fetchers take (undefined = primary).
  // Shallow-compared so the tick keeps a stable identity across unrelated
  // store writes — `usePoll` holds whatever callback it was last given, so an
  // unstable one would keep firing a stale closure.
  const subdirs = useAppStore(
    useShallow((s) => {
      const agent = s.workspace?.agents.find((a) => a.id === s.selectedAgentId);
      return agent?.repos.map((r, i) => (i === 0 ? undefined : r.subdir)) ?? [];
    }),
  );
  const tick = useCallback(async () => {
    if (!agentId) return;
    await Promise.all(subdirs.map((subdir) => fetch(agentId, subdir)));
  }, [agentId, subdirs, fetch]);
  usePoll(tick, intervalMs, [tick]);
}
