// What this window polls about git, and what it only listens to.
//
// The rule: the host emits facts on change, and a client polls only the local
// reads it is rendering right now. GitHub is the host's to read — its PR watcher
// (`supervisor::pr_watch`) sweeps every bound PR once a minute and emits
// `pr:state_changed` / `pr:checks_changed` / `pr:threads_changed` on change, and
// it fetches every project's base every five minutes (`supervisor::base_freshness`)
// — so it is read once however many windows and phones are open.
//
// Polled here (local git on the host; no GitHub), paused while the document is
// hidden (`usePoll`):
//   - fleet shortstats, 5 s — the sidebar numbers;
//   - fleet git meta, 15 s — the "base moved" chips and overlap hints, measured
//     against the base the host's loop keeps fetched;
//   - the focused agent's full git state, 1 s with the panel showing, 10 s not.
//
// Listened to (folded by `eventListeners`), with one read to seed:
//   - the fleet's PR state + CI: `loadAllPrStatus` when GitHub connects (below),
//     and on environment switch / reconnect (`environmentSwitch`) and window
//     focus (`setupResync`);
//   - the focused checkouts' PR state, checks and review threads: one
//     `get_pr_live` + `get_pr_threads` when a checkout comes into focus with
//     nothing cached for it, or when its PR changes under it (see
//     `focusedPrReads`).

import { useCallback, useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { focusedPrReads, parsePrSignature, prSignature } from "./focusedPrReads";
import { checkoutKey, splitCheckoutKey } from "./git";
import type { AppState } from "./types";

/** Mount once, at the app root. */
export function useGitSync() {
  const fetchAllShortstats = useAppStore((s) => s.fetchAllShortstats);
  const fetchAllGitMeta = useAppStore((s) => s.fetchAllGitMeta);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  const loadAllPrStatus = useAppStore((s) => s.loadAllPrStatus);

  usePoll(fetchAllShortstats, 5000, [fetchAllShortstats]);

  usePoll(fetchAllGitMeta, 15000, [fetchAllGitMeta]);

  const panelVisible = useAppStore((s) => !s.rightCollapsed && !s.activeDraftId);
  useTrackedRepoPoll(fetchGitState, panelVisible ? 1000 : 10000);

  // The fleet seed on launch: GitHub is probed asynchronously, so this runs
  // when it reports connected rather than at mount (and again if it
  // reconnects). Closed PRs get a live look here, as nothing else re-checks one
  // that reopened.
  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  useEffect(() => {
    if (githubConnected) void loadAllPrStatus(true);
  }, [githubConnected, loadAllPrStatus]);

  useFocusedPrReads(githubConnected);
}

/** The focused agent's checkouts as `checkoutKey`s — every one of its repos —
 *  sorted, so a shallow compare sees a stable array across unrelated writes. */
function focusedCheckoutKeys(s: AppState): string[] {
  const agent = s.workspace?.agents.find((a) => a.id === s.selectedAgentId);
  if (!agent) return [];
  return agent.repos
    .map((repo, i) => checkoutKey(agent.id, i === 0 ? undefined : repo.subdir))
    .sort();
}

/** Run `fetch` for every focused checkout, on `intervalMs`.
 *
 *  Covering *all* of the focused agent's repos — not just the ones a panel
 *  section happens to render — is what retired `GitPanel`'s `pollDormant`.
 *  No-ops when there's nothing to track, so callers need no guard of their
 *  own. The keys are shallow-compared because `usePoll` holds whatever
 *  callback it was last given, so an unstable one would keep firing a stale
 *  closure. */
function useTrackedRepoPoll(
  fetch: (agentId: string, subdir?: string) => Promise<void>,
  intervalMs: number,
) {
  const keys = useAppStore(useShallow(focusedCheckoutKeys));
  const tick = useCallback(async () => {
    await Promise.all(
      keys.map((key) => {
        const { agentId, subdir } = splitCheckoutKey(key);
        return fetch(agentId, subdir);
      }),
    );
  }, [keys, fetch]);
  usePoll(tick, intervalMs, [tick]);
}

/** The focused checkouts' PR signatures (see `prSignature`). */
function focusedPrSignatures(s: AppState): string[] {
  return focusedCheckoutKeys(s).map((key) => prSignature(s, key));
}

/** The focused checkouts' one-shot PR reads (see `focusedPrReads`). No timer:
 *  once a checkout's PR is in the store, the host watcher's events move it. */
function useFocusedPrReads(githubConnected: boolean) {
  const fetchPrLive = useAppStore((s) => s.fetchPrLive);
  const fetchPrThreads = useAppStore((s) => s.fetchPrThreads);
  const signatures = useAppStore(useShallow(focusedPrSignatures));
  const seen = useRef(new Map<string, number | null>());
  // Reads still out, by kind: one landing changes the signatures while the
  // other is in flight, and that must not issue the other a second time.
  const inFlight = useRef({ live: new Set<string>(), threads: new Set<string>() });
  useEffect(() => {
    // The reads gate on GitHub themselves; waiting here as well keeps a cold
    // checkout cold until they can run, so connecting later still reads it.
    if (!githubConnected) return;
    const owed = focusedPrReads(signatures.map(parsePrSignature), seen.current);
    const issue = (
      kind: "live" | "threads",
      read: (agentId: string, subdir?: string) => Promise<void>,
    ) => {
      const pending = inFlight.current[kind];
      for (const key of owed[kind]) {
        if (pending.has(key)) continue;
        pending.add(key);
        const { agentId, subdir } = splitCheckoutKey(key);
        void read(agentId, subdir).finally(() => pending.delete(key));
      }
    };
    issue("live", fetchPrLive);
    issue("threads", fetchPrThreads);
  }, [signatures, githubConnected, fetchPrLive, fetchPrThreads]);
}
