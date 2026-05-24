import { create } from "zustand";
import {
  api,
  onAgentOutput,
  onAgentStatus,
  type AgentRecord,
  type Workspace,
} from "./api";

type OutputHandler = (bytes: Uint8Array) => void;

/** Per-agent xterm sinks. Lives outside the Zustand store because it isn't
 *  reactive — components register/unregister with bare function refs. */
const outputSinks = new Map<string, OutputHandler>();

export function registerOutputSink(
  agentId: string,
  handler: OutputHandler,
): () => void {
  outputSinks.set(agentId, handler);
  return () => {
    if (outputSinks.get(agentId) === handler) outputSinks.delete(agentId);
  };
}

interface AppState {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  busy: boolean;
  lastError: string | null;
  initialized: boolean;

  init: () => Promise<void>;
  selectAgent: (id: string | null) => void;
  setRepo: (path: string, baseImage: string) => Promise<void>;
  spawn: (name: string, branch: string, task: string) => Promise<AgentRecord | null>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  clearError: () => void;
}

export const useAppStore = create<AppState>((set, get) => ({
  workspace: null,
  selectedAgentId: null,
  busy: false,
  lastError: null,
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    const workspace = await api.getWorkspace();
    set({ workspace });

    await onAgentOutput((e) => {
      const sink = outputSinks.get(e.agent_id);
      if (sink) sink(new Uint8Array(e.bytes));
    });

    await onAgentStatus((e) => {
      const ws = get().workspace;
      if (!ws) return;
      const next = {
        ...ws,
        agents: ws.agents.map((a) =>
          a.id === e.agent_id
            ? { ...a, status: e.status, last_error: e.last_error ?? a.last_error }
            : a,
        ),
      };
      set({ workspace: next });
    });
  },

  selectAgent: (id) => set({ selectedAgentId: id }),

  setRepo: async (path, baseImage) => {
    set({ busy: true, lastError: null });
    try {
      const ws = await api.setRepo(path, baseImage);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  spawn: async (name, branch, task) => {
    set({ busy: true, lastError: null });
    try {
      const rec = await api.spawnAgent(name, branch, task);
      const fresh = await api.getWorkspace();
      set({ workspace: fresh, selectedAgentId: rec.id });
      return rec;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    } finally {
      set({ busy: false });
    }
  },

  stop: async (id) => {
    try {
      await api.stopAgent(id);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  discard: async (id) => {
    try {
      await api.discardWorktree(id);
      const fresh = await api.getWorkspace();
      set((s) => ({
        workspace: fresh,
        selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
      }));
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  clearError: () => set({ lastError: null }),
}));
