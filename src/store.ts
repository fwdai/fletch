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
 *  literal from a Zustand selector triggers React's "snapshot changed"
 *  check on every render and explodes into a setState loop. */
export const EMPTY_AGENTS: readonly AgentRecord[] = Object.freeze([]);

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

export type BaseImageStatus = "unknown" | "checking" | "ready" | "missing";

interface AppState {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  /** Whether the workspace's configured base image actually exists on the
   *  user's Tart store. Drives the "build base image" banner + Spawn
   *  enablement. */
  baseImageStatus: BaseImageStatus;

  init: () => Promise<void>;
  selectAgent: (id: string | null) => void;
  setRepo: (path: string, baseImage: string) => Promise<void>;
  spawn: (name: string, branch: string, task: string) => Promise<AgentRecord | null>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  clearError: () => void;
  refreshBaseImageStatus: () => Promise<void>;
}

export const useAppStore = create<AppState>((set, get) => ({
  workspace: null,
  selectedAgentId: null,
  busy: false,
  lastError: null,
  initialized: false,
  baseImageStatus: "unknown",

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    const workspace = await api.getWorkspace();
    set({ workspace });
    void get().refreshBaseImageStatus();

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
            ? {
                ...a,
                status: e.status,
                last_error: e.last_error ?? a.last_error,
                // status_message is "live" — undefined means "no change",
                // null means "clear it" (settled into a terminal state).
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

  setRepo: async (path, baseImage) => {
    set({ busy: true, lastError: null });
    try {
      const ws = await api.setRepo(path, baseImage);
      set({ workspace: ws });
      void get().refreshBaseImageStatus();
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  refreshBaseImageStatus: async () => {
    const ws = get().workspace;
    if (!ws) {
      set({ baseImageStatus: "unknown" });
      return;
    }
    set({ baseImageStatus: "checking" });
    try {
      const list = await api.listBaseImages();
      set({
        baseImageStatus: list.includes(ws.base_image) ? "ready" : "missing",
      });
    } catch {
      // If we can't list (e.g. tart not on PATH in dev), treat as missing —
      // safer than letting the user click Spawn and get a generic error.
      set({ baseImageStatus: "missing" });
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
