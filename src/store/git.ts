import { api } from "../api";
import type { GitCommitAction } from "../components/RightPanel/primaryActions";
import { setSetting } from "../storage/settings";
import type { SliceCreator, GitSlice } from "./types";

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

  fetchPrChecks: async (agentId) => {
    try {
      const checks = await api.getPrChecks(agentId);
      // Write nulls too: null = "confirmed unavailable", distinct from the
      // absent key ("not yet fetched") that renders as the checking… state.
      set((s) => ({ prChecks: { ...s.prChecks, [agentId]: checks } }));
    } catch {
      // Non-fatal — the next poll tick retries. But a *first* fetch that
      // throws would otherwise leave the key absent and pin the panel's
      // "checking…" placeholder, so degrade it to null (mergeable-only
      // fallback). A later transient error keeps the last good value.
      set((s) =>
        agentId in s.prChecks
          ? {}
          : { prChecks: { ...s.prChecks, [agentId]: null } },
      );
    }
  },

  fetchPrComments: async (agentId) => {
    try {
      const comments = await api.getPrComments(agentId);
      set((s) => ({ prComments: { ...s.prComments, [agentId]: comments } }));
    } catch {
      // Non-fatal — the next poll tick retries. Degrade a first failure to
      // null (section omitted) rather than leaving the key absent.
      set((s) =>
        agentId in s.prComments
          ? {}
          : { prComments: { ...s.prComments, [agentId]: null } },
      );
    }
  },

  delegateGitAction: (agentId, kind, prompt) => {
    // Sent mid-turn? Then our trigger is queued behind the in-flight turn,
    // and that turn's running/settling must not be read as ours.
    const status = get().workspace?.agents.find((a) => a.id === agentId)?.status;
    const queued = status === "running";
    set((s) => ({
      gitDelegations: {
        ...s.gitDelegations,
        [agentId]: { kind, startedAt: Date.now(), sawRunning: false, queued },
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

  markGitDelegationDequeued: (agentId) => {
    set((s) => {
      const d = s.gitDelegations[agentId];
      if (!d || !d.queued) return s;
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

  pullAgent: async (agentId) => {
    try {
      await api.pullAgent(agentId);
      await get().fetchGitState(agentId);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      return false;
    }
  },

  rebaseAgent: async (agentId) => {
    try {
      await api.rebaseAgent(agentId);
      await get().fetchGitState(agentId);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      return false;
    }
  },

  commitChanges: async (agentId, message) => {
    try {
      await api.commitAgent(agentId, message);
      await get().fetchGitState(agentId);
      return true;
    } catch (e) {
      set({ lastError: String(e) });
      return false;
    }
  },

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
    try {
      await api.stashAgent(agentId);
      await get().fetchGitState(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  discardChanges: async (agentId) => {
    try {
      await api.discardAgentChanges(agentId);
      await get().fetchGitState(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  abortMerge: async (agentId) => {
    try {
      await api.abortMergeAgent(agentId);
      await get().fetchGitState(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  deleteBranch: async (agentId) => {
    try {
      await api.deleteBranchAgent(agentId);
      await get().fetchGitState(agentId);
    } catch (e) {
      set({ lastError: String(e) });
    }
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
