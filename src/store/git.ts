import { api } from "@/api";
import { gitActionProvesKind } from "@/components/RightPanel/delegation";
import type { GitCommitAction } from "@/components/RightPanel/primaryActions";
import { setSetting } from "@/storage/settings";
import type { GitSlice, SliceCreator } from "./types";

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
  prStates: {},
  prChecks: {},
  prComments: {},
  gitDelegations: {},
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
