import { create } from "zustand";
import {
  api,
  onAgentOutput,
  onAgentStatus,
  type AgentRecord,
  type Workspace,
} from "./api";

type OutputHandler = (bytes: Uint8Array) => void;

/** Stable empty-array sentinel for selectors. Returning a fresh `[]`
 *  from a Zustand selector triggers React's "snapshot changed" check
 *  on every render and explodes into a setState loop. */
export const EMPTY_AGENTS: readonly AgentRecord[] = Object.freeze([]);

const outputSinks = new Map<string, OutputHandler>();

/** Per-agent ring buffer of recent output bytes, used to repaint the
 *  terminal when the user switches tabs (which unmounts+remounts the
 *  xterm to avoid renderer-on-hidden-div crashes). Capped at ~256KB
 *  per agent so long-running sessions don't grow unbounded. */
const outputBuffers = new Map<string, Uint8Array>();
const MAX_BUFFER_BYTES = 256 * 1024;

function appendToBuffer(agentId: string, chunk: Uint8Array) {
  const existing = outputBuffers.get(agentId);
  let next: Uint8Array;
  if (!existing) {
    next = chunk;
  } else {
    next = new Uint8Array(existing.length + chunk.length);
    next.set(existing, 0);
    next.set(chunk, existing.length);
  }
  if (next.length > MAX_BUFFER_BYTES) {
    next = next.slice(next.length - MAX_BUFFER_BYTES);
  }
  outputBuffers.set(agentId, next);
}

export function getOutputBuffer(agentId: string): Uint8Array | undefined {
  return outputBuffers.get(agentId);
}

export function clearOutputBuffer(agentId: string) {
  outputBuffers.delete(agentId);
}

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
  setRepo: (path: string) => Promise<void>;
  spawn: (
    name: string,
    branch: string,
    task: string,
  ) => Promise<AgentRecord | null>;
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
      const chunk = new Uint8Array(e.bytes);
      appendToBuffer(e.agent_id, chunk);
      const sink = outputSinks.get(e.agent_id);
      if (sink) sink(chunk);
    });

    await onAgentStatus((e) => {
      const ws = get().workspace;
      if (!ws) return;
      const next = {
        ...ws,
        agents: ws.agents.map((a) =>
          a.id === e.agent_id
            ? {
                ...a,
                status: e.status,
                last_error: e.last_error ?? a.last_error,
                status_message:
                  e.status_message === undefined
                    ? a.status_message
                    : e.status_message,
              }
            : a,
        ),
      };
      set({ workspace: next });
    });
  },

  selectAgent: (id) => set({ selectedAgentId: id }),

  setRepo: async (path) => {
    set({ busy: true, lastError: null });
    try {
      const ws = await api.setRepo(path);
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
      await api.discardAgent(id);
      clearOutputBuffer(id);
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
