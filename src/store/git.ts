import { api } from "../api";
import type { GitCommitAction } from "../components/RightPanel/primaryActions";
import { setSetting } from "../storage/settings";
import type { GitSlice, SliceCreator } from "./types";

type GitSet = Parameters<SliceCreator<GitSlice>>[0];
type GitGet = Parameters<SliceCreator<GitSlice>>[1];

// Shared shape for the simple git mutations: run the backend call, refresh git
// state on success, otherwise record the error and report failure.
const runGitMutation = async (
  get: GitGet,
  agentId: string,
  fn: () => Promise<unknown>,
): Promise<boolean> => {
  try {
    await fn();
    await get().fetchGitState(agentId);
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
  fetch: (agentId: string) => Promise<GitSlice[K][string]>,
): Promise<void> => {
  try {
    const value = await fetch(agentId);
    set((s) => ({ [key]: { ...s[key], [agentId]: value } }) as Partial<GitSlice>);
  } catch {
    set((s) =>
      agentId in s[key] ? {} : ({ [key]: { ...s[key], [agentId]: null } } as Partial<GitSlice>),
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

  fetchGitState: async (agentId) => {
    try {
      const state = await api.getGitState(agentId);
      if (state) {
        set((s) => ({ gitStates: { ...s.gitStates, [agentId]: state } }));
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

  fetchPrState: async (agentId) => {
    try {
      const state = await api.getPrState(agentId);
      // Always write (including null) to distinguish "confirmed: no PR" from
      // "not yet fetched" (absent key). Unlike fetchGitState which guards the
      // write, PR state being null is meaningful.
      set((s) => ({ prStates: { ...s.prStates, [agentId]: state } }));
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

  fetchPrChecks: (agentId) => fetchPrAux(set, agentId, "prChecks", api.getPrChecks),

  fetchPrComments: (agentId) => fetchPrAux(set, agentId, "prComments", api.getPrComments),

  delegateGitAction: (agentId, kind, prompt) => {
    // Sent mid-turn? Then our trigger is queued behind the in-flight turn,
    // and that turn's running/settling must not be read as ours.
    const status = get().workspace?.agents.find((a) => a.id === agentId)?.status;
    const queued = status === "running";
    set((s) => ({
      gitDelegations: {
        ...s.gitDelegations,
        [agentId]: {
          kind,
          startedAt: Date.now(),
          sawRunning: false,
          sawGitOp: false,
          queued,
        },
      },
    }));
    void get().sendUserMessage(agentId, prompt);
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

  markGitDelegationActed: (agentId) => {
    set((s) => {
      const d = s.gitDelegations[agentId];
      // Record the git op regardless of `queued`. The backend can swap the
      // running turn for our delegated turn without an intermediate idle, so the
      // frontend may never observe the dequeue before our turn's commit/PR fires
      // its `agent:git-action` — gating on `queued` would drop the only causal
      // success signal and force a false give-up. Whether the op came from our
      // turn or one we were queued behind, a real agent mutation reached the
      // target, so pairing it with `resolved` keeps the success notice accurate.
      if (!d || d.sawGitOp) return s;
      return {
        gitDelegations: { ...s.gitDelegations, [agentId]: { ...d, sawGitOp: true } },
      };
    });
  },

  markGitDelegationDequeued: (agentId) => {
    set((s) => {
      const d = s.gitDelegations[agentId];
      if (!d?.queued) return s;
      return {
        gitDelegations: {
          ...s.gitDelegations,
          [agentId]: { ...d, queued: false, startedAt: Date.now() },
        },
      };
    });
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

  pushAgent: async (agentId) => {
    try {
      // "up-to-date" | "pushed" — lets the UI confirm the outcome.
      const summary = await api.pushAgent(agentId);
      await get().fetchGitState(agentId);
      // pr:state_changed event will update prStates automatically
      return summary;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  pullAgent: (agentId) => runGitMutation(get, agentId, () => api.pullAgent(agentId)),

  rebaseAgent: (agentId) => runGitMutation(get, agentId, () => api.rebaseAgent(agentId)),

  commitChanges: (agentId, message) =>
    runGitMutation(get, agentId, () => api.commitAgent(agentId, message)),

  commitAndOpenPr: async (agentId, message) => {
    try {
      await api.commitAgent(agentId, message);
      await api.pushAgent(agentId);
      const pr = await api.createPr(agentId, "", "");
      set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
      await get().fetchGitState(agentId);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      await get().fetchGitState(agentId);
      return false;
    }
  },

  stashChanges: async (agentId) => {
    await runGitMutation(get, agentId, () => api.stashAgent(agentId));
  },

  discardChanges: async (agentId) => {
    await runGitMutation(get, agentId, () => api.discardAgentChanges(agentId));
  },

  abortMerge: async (agentId) => {
    await runGitMutation(get, agentId, () => api.abortMergeAgent(agentId));
  },

  deleteBranch: async (agentId) => {
    await runGitMutation(get, agentId, () => api.deleteBranchAgent(agentId));
  },

  createPr: async (agentId, title, body) => {
    try {
      const pr = await api.createPr(agentId, title, body);
      set((s) => ({ prStates: { ...s.prStates, [agentId]: pr } }));
      return pr;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    }
  },

  mergePr: async (agentId) => {
    try {
      await api.mergePr(agentId);
      // Refresh immediately: no backend event fires on merge, and the panel
      // should transition to the merged state as soon as GitHub reports it
      // (with --auto + pending checks the PR can legitimately stay open).
      await get().fetchPrState(agentId);
      await get().fetchPrChecks(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
});
