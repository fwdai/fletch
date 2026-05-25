import { create } from "zustand";
import {
  api,
  onAgentBranch,
  onAgentEvent,
  onAgentOutput,
  onAgentStatus,
  onAgentTask,
  onAgentView,
  type AgentRecord,
  type AgentView,
  type Workspace,
} from "./api";

type OutputHandler = (bytes: Uint8Array) => void;

export const EMPTY_AGENTS: readonly AgentRecord[] = Object.freeze([]);

const outputSinks = new Map<string, OutputHandler>();

// ---- Per-agent PTY output buffer ----------------------------------------
// Used by native view to repaint after tab switch / view switch.
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

// ---- Custom-view structured message log ---------------------------------

export type ManagedItem =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string; streaming?: boolean }
  | { kind: "tool_use"; id: string; name: string; input: unknown }
  | {
      kind: "tool_result";
      tool_use_id: string;
      content: unknown;
      is_error?: boolean;
    }
  | { kind: "system"; text: string }
  | { kind: "result"; text: string; is_error?: boolean };

interface AppState {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  managedLogs: Record<string, ManagedItem[]>;
  /** True between user sending a turn and claude's `result` event for
   *  that turn. Drives the send-button disabled state and the
   *  "thinking…" indicator. */
  managedBusy: Record<string, boolean>;
  /** True while a view switch is in flight — disable toggle UI. */
  switchInFlight: Record<string, boolean>;
  /** Most recent turn's `usage.input_tokens` — matches claude's
   *  `/context`. Populated from the stream-json `result` event;
   *  custom-view only (native view doesn't deliver structured events).
   *  In-memory only — resets to empty on app restart. */
  tokens: Record<string, number>;

  init: () => Promise<void>;
  selectAgent: (id: string | null) => void;
  setRepo: (path: string) => Promise<void>;
  spawn: (view: AgentView) => Promise<AgentRecord | null>;
  sendUserMessage: (id: string, text: string) => Promise<void>;
  switchView: (id: string, view: AgentView) => Promise<void>;
  resume: (id: string) => Promise<void>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  clearError: () => void;
}

function appendItem(
  state: AppState,
  agentId: string,
  item: ManagedItem,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  return { managedLogs: { ...state.managedLogs, [agentId]: [...list, item] } };
}

function updateLastAssistantStreaming(
  state: AppState,
  agentId: string,
  appendText: string,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  const lastIdx = list.length - 1;
  const last = list[lastIdx];
  if (last && last.kind === "assistant" && last.streaming) {
    const next = [...list];
    next[lastIdx] = { ...last, text: last.text + appendText };
    return { managedLogs: { ...state.managedLogs, [agentId]: next } };
  }
  return appendItem(state, agentId, {
    kind: "assistant",
    text: appendText,
    streaming: true,
  });
}

function finalizeStreamingAssistant(
  state: AppState,
  agentId: string,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  const lastIdx = list.length - 1;
  const last = list[lastIdx];
  if (last && last.kind === "assistant" && last.streaming) {
    const next = [...list];
    next[lastIdx] = { ...last, streaming: false };
    return { managedLogs: { ...state.managedLogs, [agentId]: next } };
  }
  return {};
}

function mergePatches(
  a: Partial<AppState>,
  b: Partial<AppState>,
): Partial<AppState> {
  return {
    ...a,
    ...b,
    managedLogs: {
      ...(a.managedLogs ?? {}),
      ...(b.managedLogs ?? {}),
    },
  };
}

function handleManagedEvent(
  state: AppState,
  agentId: string,
  ev: Record<string, unknown>,
): Partial<AppState> {
  const type = ev.type as string | undefined;

  if (type === "stream_event") {
    const inner = (ev.event ?? {}) as Record<string, unknown>;
    const delta = (inner.delta ?? {}) as Record<string, unknown>;
    if (delta.type === "text_delta" && typeof delta.text === "string") {
      return updateLastAssistantStreaming(state, agentId, delta.text);
    }
    return {};
  }

  if (type === "assistant") {
    const message = (ev.message ?? {}) as Record<string, unknown>;
    const content = (message.content ?? []) as Array<Record<string, unknown>>;
    let patches: Partial<AppState> = {};
    patches = mergePatches(patches, finalizeStreamingAssistant(state, agentId));
    let working: AppState = { ...state, ...patches } as AppState;
    for (const block of content) {
      if (block.type === "text" && typeof block.text === "string") {
        const list = working.managedLogs[agentId] ?? [];
        const last = list[list.length - 1];
        if (
          !(last && last.kind === "assistant" && last.text === block.text)
        ) {
          const p = appendItem(working, agentId, {
            kind: "assistant",
            text: block.text,
          });
          working = { ...working, ...p } as AppState;
          patches = mergePatches(patches, p);
        }
      } else if (block.type === "tool_use") {
        const p = appendItem(working, agentId, {
          kind: "tool_use",
          id: String(block.id ?? ""),
          name: String(block.name ?? "tool"),
          input: block.input,
        });
        working = { ...working, ...p } as AppState;
        patches = mergePatches(patches, p);
      }
    }
    return patches;
  }

  if (type === "user") {
    const message = (ev.message ?? {}) as Record<string, unknown>;
    const content = message.content;
    if (Array.isArray(content)) {
      let patches: Partial<AppState> = {};
      let working: AppState = state;
      for (const block of content as Array<Record<string, unknown>>) {
        if (block.type === "tool_result") {
          const p = appendItem(working, agentId, {
            kind: "tool_result",
            tool_use_id: String(block.tool_use_id ?? ""),
            content: block.content,
            is_error: block.is_error === true,
          });
          working = { ...working, ...p } as AppState;
          patches = mergePatches(patches, p);
        }
      }
      return patches;
    }
    return {};
  }

  if (type === "result") {
    const subtype = String(ev.subtype ?? "");
    const isError = subtype !== "success";
    const text =
      typeof ev.result === "string"
        ? ev.result
        : `Turn complete (${subtype || "ok"})`;
    let patches = finalizeStreamingAssistant(state, agentId);
    const working = { ...state, ...patches } as AppState;
    patches = mergePatches(
      patches,
      appendItem(working, agentId, {
        kind: "result",
        text,
        is_error: isError,
      }),
    );
    // Pull the context-window size from `usage.input_tokens` — this is
    // what claude's `/context` displays.
    const inputTokens = (ev.usage as Record<string, unknown> | undefined)
      ?.input_tokens;
    const tokens =
      typeof inputTokens === "number" && inputTokens > 0
        ? { ...state.tokens, [agentId]: inputTokens }
        : state.tokens;
    return {
      ...patches,
      managedBusy: { ...state.managedBusy, [agentId]: false },
      tokens,
    };
  }

  // system / init / unknown — ignore in UI
  return {};
}

export const useAppStore = create<AppState>((set, get) => ({
  workspace: null,
  selectedAgentId: null,
  busy: false,
  lastError: null,
  initialized: false,
  managedLogs: {},
  managedBusy: {},
  switchInFlight: {},
  tokens: {},

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    await onAgentOutput((e) => {
      const chunk = new Uint8Array(e.bytes);
      appendToBuffer(e.agent_id, chunk);
      const sink = outputSinks.get(e.agent_id);
      if (sink) sink(chunk);
    });

    await onAgentEvent((e) => {
      set((state) => handleManagedEvent(state, e.agent_id, e.event));
    });

    await onAgentBranch((e) => {
      const ws = get().workspace;
      if (!ws) return;
      set({
        workspace: {
          ...ws,
          agents: ws.agents.map((a) =>
            a.id === e.agent_id ? { ...a, branch: e.branch } : a,
          ),
        },
      });
    });

    await onAgentTask((e) => {
      const ws = get().workspace;
      if (!ws) return;
      set({
        workspace: {
          ...ws,
          agents: ws.agents.map((a) =>
            a.id === e.agent_id ? { ...a, task: e.task } : a,
          ),
        },
      });
    });

    await onAgentView((e) => {
      // Backend changed the view (usually our own switchView call).
      // Reflect on the record so the UI swaps components.
      const ws = get().workspace;
      if (!ws) return;
      set({
        workspace: {
          ...ws,
          agents: ws.agents.map((a) =>
            a.id === e.agent_id ? { ...a, view: e.view } : a,
          ),
        },
      });
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
              }
            : a,
        ),
      };
      set({ workspace: next });
    });

    const workspace = await api.getWorkspace();
    set({ workspace });
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

  spawn: async (view) => {
    set({ busy: true, lastError: null });
    try {
      const rec = await api.spawnAgent(view);
      const fresh = await api.getWorkspace();
      set((state) => {
        const patches: Partial<AppState> = {
          workspace: fresh,
          selectedAgentId: rec.id,
        };
        if (view === "custom") {
          // No initial task / no branch yet — both arrive after the
          // user's first message. Greet with what we have (the
          // worktree id and the parent branch we forked from).
          const parent = rec.parent_branch
            ? ` from ${rec.parent_branch}`
            : "";
          const greeting =
            `Worktree ${rec.id} ready${parent}. ` +
            `Claude is waiting — send a message to begin.`;
          patches.managedLogs = {
            ...state.managedLogs,
            [rec.id]: [{ kind: "system", text: greeting }],
          };
          // No turn is in flight at spawn — input is enabled.
          patches.managedBusy = { ...state.managedBusy, [rec.id]: false };
        }
        return patches;
      });
      return rec;
    } catch (e) {
      set({ lastError: String(e) });
      return null;
    } finally {
      set({ busy: false });
    }
  },

  sendUserMessage: async (id, text) => {
    try {
      set((state) => ({
        managedLogs: {
          ...state.managedLogs,
          [id]: [
            ...(state.managedLogs[id] ?? []),
            { kind: "user", text },
          ],
        },
        managedBusy: { ...state.managedBusy, [id]: true },
      }));
      await api.sendUserMessage(id, text);
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  switchView: async (id, view) => {
    // Important: `claude --print --resume` in stream-json input mode
    // does NOT replay history on stdout — it loads the session and
    // waits silently for the next stdin message. So we must NOT clear
    // managedLogs on a switch — the log is the only record the user
    // has of prior turns in the custom view.
    //
    // The PTY output buffer, on the other hand, *is* re-drawn fresh
    // by claude's TUI on resume, so we clear it on switch-to-native
    // to avoid stale frames stacking on top of the new render.
    if (view === "native") {
      clearOutputBuffer(id);
    }
    set((state) => ({
      // Reset busy so the custom view's input box is enabled
      // immediately after a switch (we're no longer waiting on a
      // pre-switch turn).
      managedBusy: { ...state.managedBusy, [id]: false },
      switchInFlight: { ...state.switchInFlight, [id]: true },
    }));
    try {
      await api.switchView(id, view);
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set((state) => ({
        switchInFlight: { ...state.switchInFlight, [id]: false },
      }));
    }
  },

  resume: async (id) => {
    // Same buffer-clearing logic as a view switch — claude's --resume
    // doesn't replay events on stdout (custom view stays as-is), but
    // its PTY redraws history from scratch on resume, so native's
    // output buffer should start empty to avoid duplicated frames.
    clearOutputBuffer(id);
    set((state) => ({
      managedBusy: { ...state.managedBusy, [id]: false },
    }));
    try {
      await api.resumeAgent(id);
    } catch (e) {
      set({ lastError: String(e) });
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
      set((s) => {
        const { [id]: _droppedLog, ...restLogs } = s.managedLogs;
        const { [id]: _droppedBusy, ...restBusy } = s.managedBusy;
        const { [id]: _droppedTokens, ...restTokens } = s.tokens;
        return {
          workspace: fresh,
          selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
          managedLogs: restLogs,
          managedBusy: restBusy,
          tokens: restTokens,
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  clearError: () => set({ lastError: null }),
}));
