import {
  api,
  type GitMeta,
  type GitState,
  type PrChecks,
  type PrComments,
  type PrState,
  type ShortStats,
  type VerificationReport,
} from "@/api";
import {
  type GitDelegation,
  type GitDelegationKind,
  gitActionProvesKind,
} from "@/components/RightPanel/delegation";
import type { GitCommitAction } from "@/components/RightPanel/primaryActions";
import { setSetting } from "@/storage/settings";
import type { SliceCreator } from "./types";

export interface GitSlice {
  /** Full git state — branch, ahead/behind, file list, totals. Keyed by
   *  `gitKey(agentId, subdir?)` (store/git.ts): the plain agent_id addresses
   *  the primary repo, `agentId::subdir` a secondary repo of a multi-repo
   *  agent. Only populated for the focused agent (by GitPanel's 1s poll
   *  while it's mounted). For sidebar shortstats / right-rail badges of
   *  other agents, read from `gitShortstats` instead. */
  gitStates: Record<string, GitState>;
  /** Compact per-agent shortstats (additions / deletions / file count),
   *  keyed by agent_id. Updated for every live agent on the app-wide 5s
   *  poll — kept in its own map so the focused agent's richer `gitStates`
   *  entry isn't clobbered by a slower bulk reply. */
  gitShortstats: Record<string, ShortStats>;
  /** Advisory per-checkout git metadata (base staleness + changed-file paths),
   *  keyed by `gitKey(agentId, subdir?)`. Fed by the app-wide `getAllGitMeta`
   *  poll — separate from `gitShortstats` (badge numbers) so each evolves on its
   *  own cadence. Drives the "base moved" staleness chips and overlap hints. */
  gitMeta: Record<string, GitMeta>;
  /** PR state, keyed by `gitKey(agentId, subdir?)` — plain agent_id for the
   *  primary repo (updated by the pr:state_changed watcher event + bulk
   *  polls), `agentId::subdir` for a secondary repo. */
  prStates: Record<string, PrState | null>;
  /** Rich PR merge-gate + checks, keyed by `gitKey(agentId, subdir?)`. Absent
   *  key = not yet fetched; `null` = confirmed unavailable (no PR / gh
   *  failure). */
  prChecks: Record<string, PrChecks | null>;
  /** Unresolved PR review comments, keyed by `gitKey(agentId, subdir?)`.
   *  Absent = not yet fetched; `null` = confirmed unavailable (no PR / gh
   *  failure). */
  prComments: Record<string, PrComments | null>;
  /** Active agent-delegated git action per agent (absent = none). Set when a
   *  panel action hands control to the agent; cleared by the panel when the
   *  watched git/PR transition lands or the agent gives up. */
  gitDelegations: Record<string, GitDelegation>;
  /** Latest turn-end verification report per agent (keyed by agent_id), from
   *  the opt-in `verify:report` event. Feeds the Mission Control card's tests
   *  chip. Absent = never verified (no chip). */
  verificationReports: Record<string, VerificationReport>;
  /** Sticky changes-state commit mode (Commit / & push / & open PR). Global
   *  across workspaces, persisted in settings until the user picks another. */
  gitCommitAction: GitCommitAction;

  /** Fetch full git state for one agent (used by the focused panel's poll).
   *  `subdir` targets a secondary repo of a multi-repo agent (stored under
   *  `gitKey(agentId, subdir)`); omitted = the primary repo, plain key. */
  fetchGitState: (agentId: string, subdir?: string) => Promise<void>;
  /** Fetch compact shortstats for every live agent in one round-trip
   *  (used by the app-wide background poll). */
  fetchAllShortstats: () => Promise<void>;
  /** Fetch advisory git metadata (base staleness + file paths) for every live
   *  checkout in one round-trip (app-wide background poll, local git only). */
  fetchAllGitMeta: () => Promise<void>;
  /** Slow-cadence host-side fetch of each project's base branch on its source
   *  repo, so `fetchAllGitMeta` can measure staleness against a moved base.
   *  Network + GitHub-gated; silent (never surfaces an error). */
  refreshBaseFreshness: () => Promise<void>;
  fetchPrState: (agentId: string, subdir?: string) => Promise<void>;
  /** Refresh PR state for every repo with a known PR across every agent in
   *  one round-trip (used by the app-wide background poll). The reply is
   *  keyed by `gitKey`, so a multi-repo agent's secondary-repo PRs land in
   *  the store too and the sidebar badge updates without opening the panel. */
  refreshAllPrStates: () => Promise<void>;
  /** Refresh CI checks for every repo with an open PR in one round-trip
   *  (used by the app-wide background poll, keyed by `gitKey` like
   *  `refreshAllPrStates`) so the sidebar PR pill can tint pass/fail without
   *  opening the Git panel. */
  refreshAllPrChecks: () => Promise<void>;
  fetchPrChecks: (agentId: string, subdir?: string) => Promise<void>;
  fetchPrComments: (agentId: string, subdir?: string) => Promise<void>;
  delegateGitAction: (
    agentId: string,
    kind: GitDelegationKind,
    prompt: string,
    /** Target checkout of a multi-repo agent; undefined = primary. */
    subdir?: string,
  ) => void;
  markGitDelegationRunning: (agentId: string) => void;
  /** The agent ran a successful mutating git op `op` (backend
   *  `agent:git-action`). Sets the causal proof only if `op` matches the
   *  pending delegation's kind. */
  markGitDelegationActed: (agentId: string, op: string) => void;
  /** The pre-existing turn the delegation was queued behind has settled —
   *  drop `queued` and restart the give-up clock for our own turn. */
  markGitDelegationDequeued: (agentId: string) => void;
  clearGitDelegation: (agentId: string) => void;
  setGitCommitAction: (action: GitCommitAction) => void;
  /** Resolves to "up-to-date" | "pushed" on success, null on error. */
  pushAgent: (agentId: string, subdir?: string) => Promise<string | null>;
  /** Resolves true on success, false on error. */
  pullAgent: (agentId: string, subdir?: string) => Promise<boolean>;
  /** Resolves true on success, false on error. */
  rebaseAgent: (agentId: string, subdir?: string) => Promise<boolean>;
  commitChanges: (agentId: string, message: string, subdir?: string) => Promise<boolean>;
  /** Commit all changes, push, and open a PR — the "Commit & open PR"
   *  primary CTA wired from the git panel. Returns false on any step
   *  failure so the UI can leave the textarea content in place. */
  commitAndOpenPr: (agentId: string, message: string, subdir?: string) => Promise<boolean>;
  stashChanges: (agentId: string, subdir?: string) => Promise<void>;
  discardChanges: (agentId: string, subdir?: string) => Promise<void>;
  abortMerge: (agentId: string, subdir?: string) => Promise<void>;
  deleteBranch: (agentId: string, subdir?: string) => Promise<void>;
  createPr: (
    agentId: string,
    title: string,
    body: string,
    subdir?: string,
  ) => Promise<PrState | null>;
  mergePr: (agentId: string, subdir?: string) => Promise<void>;
  /** Publish a local-only project (no origin) to GitHub, then refresh git
   *  state so the panel switches out of the no-origin affordances. Resolves
   *  the repo web URL on success, null on error. */
  publishAgent: (agentId: string, isPrivate: boolean) => Promise<string | null>;
}

type GitSet = Parameters<SliceCreator<GitSlice>>[0];
type GitGet = Parameters<SliceCreator<GitSlice>>[1];

/** Composite key for the per-repo git/PR maps (`gitStates`, `prStates`,
 *  `prChecks`, `prComments`). The primary repo (no `subdir`) keeps the plain
 *  agent id — the key every existing write path (background bulk polls, tauri
 *  event reducers, sidebar badges) uses — so only secondary-repo fetches get
 *  the suffixed form. */
export function gitKey(agentId: string, subdir?: string): string {
  return subdir ? `${agentId}::${subdir}` : agentId;
}

/** Max `behind` across an agent's checkouts (the `agentId` primary key plus
 *  every `agentId::subdir` secondary in `gitMeta`), or null when every base is
 *  unknown or fresh — a stale secondary must surface even when the primary is
 *  current. */
export function maxBehind(meta: Record<string, GitMeta>, agentId: string): number | null {
  const prefix = `${agentId}::`;
  let worst: number | null = null;
  for (const [key, m] of Object.entries(meta)) {
    if (key !== agentId && !key.startsWith(prefix)) continue;
    if (m.behind == null || m.behind <= 0) continue;
    if (worst == null || m.behind > worst) worst = m.behind;
  }
  return worst;
}

// Shared shape for the simple git mutations: run the backend call, refresh git
// state on success, otherwise record the error and report failure.
const runGitMutation = async (
  get: GitGet,
  agentId: string,
  fn: () => Promise<unknown>,
  subdir?: string,
): Promise<boolean> => {
  try {
    await fn();
    await get().fetchGitState(agentId, subdir);
    return true;
  } catch (e) {
    get().setLastError(String(e));
    return false;
  }
};

// fetchPrChecks/fetchPrComments are identical except for the slice key and the
// backend call: write the value (including null = "confirmed unavailable") on
// success; on a *first* failure degrade the absent key to null so the panel
// drops the "checking…" placeholder, while a later transient error keeps the
// last good value.
const fetchPrAux = async <K extends "prChecks" | "prComments">(
  set: GitSet,
  agentId: string,
  key: K,
  fetch: (agentId: string, subdir?: string) => Promise<GitSlice[K][string]>,
  subdir?: string,
): Promise<void> => {
  const mapKey = gitKey(agentId, subdir);
  try {
    const value = await fetch(agentId, subdir);
    set((s) => ({ [key]: { ...s[key], [mapKey]: value } }) as Partial<GitSlice>);
  } catch {
    set((s) =>
      mapKey in s[key] ? {} : ({ [key]: { ...s[key], [mapKey]: null } } as Partial<GitSlice>),
    );
  }
};

export const createGitSlice: SliceCreator<GitSlice> = (set, get) => ({
  gitStates: {},
  gitShortstats: {},
  gitMeta: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  gitDelegations: {},
  verificationReports: {},
  gitCommitAction: "agent-commit-pr" as GitCommitAction,

  fetchGitState: async (agentId, subdir) => {
    try {
      const state = await api.getGitState(agentId, subdir);
      if (state) {
        set((s) => ({ gitStates: { ...s.gitStates, [gitKey(agentId, subdir)]: state } }));
      }
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchAllShortstats: async () => {
    try {
      const map = await api.getAllShortstats();
      // Replace wholesale — agents archived/removed between ticks fall
      // out naturally. This map is independent of `gitStates`, so the
      // focused panel's full-state poll can't be clobbered.
      set({ gitShortstats: map });
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchAllGitMeta: async () => {
    try {
      const map = await api.getAllGitMeta();
      // Replace wholesale (like gitShortstats) — agents removed between ticks
      // fall out naturally, and this map is independent of gitStates so the
      // focused panel's full-state poll is never clobbered.
      set({ gitMeta: map });
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  refreshBaseFreshness: async () => {
    try {
      await api.refreshBaseFreshness();
    } catch {
      // Background fetch — silent by contract; the next tick retries.
    }
  },

  fetchPrState: async (agentId, subdir) => {
    try {
      const state = await api.getPrState(agentId, subdir);
      // Always write (including null) to distinguish "confirmed: no PR" from
      // "not yet fetched" (absent key). Unlike fetchGitState which guards the
      // write, PR state being null is meaningful.
      set((s) => ({ prStates: { ...s.prStates, [gitKey(agentId, subdir)]: state } }));
    } catch {
      // non-fatal
    }
  },

  refreshAllPrStates: async () => {
    try {
      // Set state directly from the reply (rather than via `pr:state_changed`
      // events) so the very first poll — which usePoll fires immediately on
      // mount — can't race the store's event listener finishing its async
      // attach during init(). Merge so agents without a known PR keep whatever
      // state the focused-panel / per-trigger paths recorded.
      const map = await api.refreshAllPrStates();
      set((s) => ({ prStates: { ...s.prStates, ...map } }));
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  refreshAllPrChecks: async () => {
    try {
      // Merge (like refreshAllPrStates) so the focused panel's per-agent
      // checks poll isn't clobbered between bulk ticks. The batched reply only
      // carries resolved rollups, so absent agents keep their last value rather
      // than being wiped.
      const map = await api.refreshAllPrChecks();
      set((s) => ({ prChecks: { ...s.prChecks, ...map } }));
    } catch {
      // non-fatal — next poll tick will retry
    }
  },

  fetchPrChecks: (agentId, subdir) => fetchPrAux(set, agentId, "prChecks", api.getPrChecks, subdir),

  fetchPrComments: (agentId, subdir) =>
    fetchPrAux(set, agentId, "prComments", api.getPrComments, subdir),

  delegateGitAction: (agentId, kind, prompt, subdir) => {
    // If the agent is already running, DON'T inject the trigger mid-turn: Claude
    // coalesces a stdin message into the current turn (it wouldn't run as its
    // own turn), and the turn boundary isn't observable, so we couldn't tell our
    // turn's git ops from the in-flight turn's. Instead hold the trigger and
    // deliver it once the agent goes idle (markGitDelegationDequeued) — then the
    // delegated turn runs in isolation and its git-action is unambiguously ours.
    const status = get().workspace?.agents.find((a) => a.id === agentId)?.status;
    const queued = status === "running";
    set((s) => ({
      gitDelegations: {
        ...s.gitDelegations,
        [agentId]: {
          kind,
          prompt,
          startedAt: Date.now(),
          sawRunning: false,
          sawGitOp: false,
          queued,
          subdir,
        },
      },
    }));
    if (!queued) void get().sendUserMessage(agentId, prompt);
  },

  markGitDelegationRunning: (agentId) => {
    set((s) => {
      const d = s.gitDelegations[agentId];
      if (!d || d.sawRunning) return s;
      return {
        gitDelegations: { ...s.gitDelegations, [agentId]: { ...d, sawRunning: true } },
      };
    });
  },

  markGitDelegationActed: (agentId, op) => {
    set((s) => {
      const d = s.gitDelegations[agentId];
      // Ignore ops while `queued`: our trigger hasn't been delivered yet, so any
      // git-action belongs to the turn we're waiting behind. (`delegateGitAction`
      // defers delivery until idle, so this is reliable — by the time we drop
      // `queued` the prior turn has ended.) Then require an op from this
      // delegation's own playbook (kind-match), so even within our turn an
      // unrelated mutation can't stand in. Paired with `resolved` in
      // delegationStep, that ties success to the agent doing the requested work.
      if (!d || d.queued || d.sawGitOp || !gitActionProvesKind(d.kind, op)) return s;
      return {
        gitDelegations: { ...s.gitDelegations, [agentId]: { ...d, sawGitOp: true } },
      };
    });
  },

  markGitDelegationDequeued: (agentId) => {
    // The turn we were queued behind has ended — NOW deliver the held trigger so
    // our delegated turn runs in isolation, and start the give-up clock from
    // here. Capture the prompt inside the atomic flip so only the call that
    // actually dequeues sends (no double-delivery from repeated effect ticks).
    let toSend: string | null = null;
    set((s) => {
      const d = s.gitDelegations[agentId];
      if (!d?.queued) return s;
      toSend = d.prompt;
      return {
        gitDelegations: {
          ...s.gitDelegations,
          [agentId]: { ...d, queued: false, startedAt: Date.now() },
        },
      };
    });
    if (toSend !== null) void get().sendUserMessage(agentId, toSend);
  },

  clearGitDelegation: (agentId) => {
    set((s) => {
      const { [agentId]: _dropped, ...rest } = s.gitDelegations;
      return { gitDelegations: rest };
    });
  },

  setGitCommitAction: (action) => {
    set({ gitCommitAction: action });
    void setSetting("gitCommitAction", action);
  },

  pushAgent: async (agentId, subdir) => {
    try {
      // "up-to-date" | "pushed" — lets the UI confirm the outcome.
      const summary = await api.pushAgent(agentId, subdir);
      await get().fetchGitState(agentId, subdir);
      // pr:state_changed event will update prStates automatically
      return summary;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  pullAgent: (agentId, subdir) =>
    runGitMutation(get, agentId, () => api.pullAgent(agentId, subdir), subdir),

  rebaseAgent: (agentId, subdir) =>
    runGitMutation(get, agentId, () => api.rebaseAgent(agentId, subdir), subdir),

  commitChanges: (agentId, message, subdir) =>
    runGitMutation(get, agentId, () => api.commitAgent(agentId, message, subdir), subdir),

  commitAndOpenPr: async (agentId, message, subdir) => {
    try {
      await api.commitAgent(agentId, message, subdir);
      await api.pushAgent(agentId, subdir);
      const pr = await api.createPr(agentId, "", "", subdir);
      set((s) => ({ prStates: { ...s.prStates, [gitKey(agentId, subdir)]: pr } }));
      await get().fetchGitState(agentId, subdir);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      await get().fetchGitState(agentId, subdir);
      return false;
    }
  },

  stashChanges: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.stashAgent(agentId, subdir), subdir);
  },

  discardChanges: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.discardAgentChanges(agentId, subdir), subdir);
  },

  abortMerge: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.abortMergeAgent(agentId, subdir), subdir);
  },

  deleteBranch: async (agentId, subdir) => {
    await runGitMutation(get, agentId, () => api.deleteBranchAgent(agentId, subdir), subdir);
  },

  createPr: async (agentId, title, body, subdir) => {
    try {
      const pr = await api.createPr(agentId, title, body, subdir);
      set((s) => ({ prStates: { ...s.prStates, [gitKey(agentId, subdir)]: pr } }));
      return pr;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  mergePr: async (agentId, subdir) => {
    try {
      await api.mergePr(agentId, subdir);
      // Refresh immediately: no backend event fires on merge, and the panel
      // should transition to the merged state as soon as GitHub reports it
      // (with --auto + pending checks the PR can legitimately stay open).
      await get().fetchPrState(agentId, subdir);
      await get().fetchPrChecks(agentId, subdir);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  publishAgent: async (agentId, isPrivate) => {
    try {
      const url = await api.publishAgent(agentId, isPrivate);
      // Origin now exists — refresh so the panel drops the no-origin
      // affordances and shows normal push/PR against the new remote.
      await get().fetchGitState(agentId);
      return url;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },
});
