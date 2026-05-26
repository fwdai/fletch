import { create } from "zustand";
import {
  api,
  onAgentBranch,
  onAgentEvent,
  onAgentOutput,
  onAgentRepoAdded,
  onAgentStatus,
  onAgentTask,
  onAgentView,
  type AgentRecord,
  type AgentView,
  type Workspace,
} from "./api";
import { DEFAULT_PROVIDER_ID } from "./data/providers";
import { LANDMARK_NAMES, pickLandmark } from "./data/landmarks";

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

function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

function asRecord(v: unknown): Record<string, unknown> {
  return isRecord(v) ? v : {};
}

function asBlockList(v: unknown): Array<Record<string, unknown>> {
  return Array.isArray(v) ? v.filter(isRecord) : [];
}

function contentText(content: unknown): string {
  if (typeof content === "string") return content;
  return asBlockList(content)
    .map((block) => {
      if (block.type === "text" && typeof block.text === "string") {
        return block.text;
      }
      return "";
    })
    .filter(Boolean)
    .join("\n");
}

function findLastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i -= 1) {
    if (predicate(items[i])) return i;
  }
  return -1;
}

// ---- Drafts ----------------------------------------------------------------
// A draft is a new agent the user is about to spawn. It owns a landmark
// name + chosen provider + base branch; the first message in the
// composer spawns the real agent and sends the prompt.

export interface DraftAgent {
  id: string;
  /** Repo (sidebar group) this draft lives under. */
  repoPath: string;
  /** Rolled landmark name; user can re-roll before sending. */
  name: string;
  /** Provider id (mocked — only "claude" currently spawns anything). */
  provider: string;
  /** Base branch to fork from. */
  base: string;
}

// ---- Feature flags & appearance --------------------------------------------

export type ThemeMode = "dark" | "light";
export type Density = "comfortable" | "compact";
export type WorkspaceView = "custom" | "native";

export interface FeatureFlags {
  git: boolean;
  diff: boolean;
  run: boolean;
  terminal: boolean;
  thinkingBudget: boolean;
  autoEdit: boolean;
  statusBar: boolean;
  tokenUsage: boolean;
}

const DEFAULT_FEATURES: FeatureFlags = {
  git: true,
  diff: false,
  run: false,
  terminal: false,
  thinkingBudget: true,
  autoEdit: false,
  statusBar: false,
  tokenUsage: false,
};

const FEATURE_KEYS = Object.keys(DEFAULT_FEATURES) as (keyof FeatureFlags)[];

function loadFeatures(): FeatureFlags {
  try {
    const raw = localStorage.getItem("quorum:features");
    if (!raw) return DEFAULT_FEATURES;
    const parsed = JSON.parse(raw) as Partial<FeatureFlags>;
    return { ...DEFAULT_FEATURES, ...parsed };
  } catch {
    return DEFAULT_FEATURES;
  }
}

function loadProviderFlags(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem("quorum:providers");
    if (!raw) return {};
    return JSON.parse(raw) as Record<string, boolean>;
  } catch {
    return {};
  }
}

function loadString<T extends string>(key: string, fallback: T): T {
  const v = localStorage.getItem(key);
  return (v as T) || fallback;
}

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
   *  `/context`. */
  tokens: Record<string, number>;

  // ── ephemeral UI state ────────────────────────────────────────────────────
  drafts: DraftAgent[];
  activeDraftId: string | null;
  settingsOpen: boolean;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftWidth: number;
  rightWidth: number;

  // ── appearance & feature flags ────────────────────────────────────────────
  theme: ThemeMode;
  accent: string;
  density: Density;
  showLandmarks: boolean;
  features: FeatureFlags;
  providerFlags: Record<string, boolean>;
  /** View mode preference for the workspace pane. Persisted; falls
   *  back to the agent's own `view` field for native vs. custom
   *  switching. */
  viewMode: WorkspaceView;

  // ── actions ────────────────────────────────────────────────────────────────
  init: () => Promise<void>;
  selectAgent: (id: string | null) => void;
  addWorkspaceRepo: (path: string) => Promise<void>;
  removeWorkspaceRepo: (path: string) => Promise<void>;
  spawn: (view: AgentView, repoPath: string) => Promise<AgentRecord | null>;
  sendUserMessage: (id: string, text: string) => Promise<void>;
  switchView: (id: string, view: AgentView) => Promise<void>;
  resume: (id: string) => Promise<void>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  clearError: () => void;

  // drafts
  createDraft: (repoPath: string) => void;
  updateDraft: (id: string, patch: Partial<DraftAgent>) => void;
  removeDraft: (id: string) => void;
  selectDraft: (id: string | null) => void;
  rerollDraftName: (id: string) => void;
  /** Spawn the real agent for a draft and dispatch the first message. */
  spawnFromDraft: (id: string, text: string, provider: string) => Promise<void>;

  // UI
  toggleSettings: (open?: boolean) => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;

  // appearance
  setTheme: (t: ThemeMode) => void;
  setAccent: (a: string) => void;
  setDensity: (d: Density) => void;
  setShowLandmarks: (v: boolean) => void;
  setFeature: <K extends keyof FeatureFlags>(k: K, v: FeatureFlags[K]) => void;
  setProviderEnabled: (id: string, enabled: boolean) => void;
  setViewMode: (v: WorkspaceView) => void;
}

function appendItem(
  state: AppState,
  agentId: string,
  item: ManagedItem,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  return { managedLogs: { ...state.managedLogs, [agentId]: [...list, item] } };
}

function appendUserIfMissing(
  state: AppState,
  agentId: string,
  text: string,
): Partial<AppState> {
  const trimmed = text.trim();
  if (!trimmed) return {};
  const list = state.managedLogs[agentId] ?? [];
  if (list.some((item) => item.kind === "user" && item.text === trimmed)) {
    return {};
  }
  return appendItem(state, agentId, { kind: "user", text: trimmed });
}

function upsertToolUse(
  state: AppState,
  agentId: string,
  tool: Extract<ManagedItem, { kind: "tool_use" }>,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  const idx = list.findIndex(
    (item) => item.kind === "tool_use" && item.id === tool.id,
  );
  if (idx === -1) return appendItem(state, agentId, tool);
  const next = [...list];
  next[idx] = { ...tool };
  return { managedLogs: { ...state.managedLogs, [agentId]: next } };
}

function updateToolInputDelta(
  state: AppState,
  agentId: string,
  index: number,
  partialJson: string,
): Partial<AppState> {
  const list = state.managedLogs[agentId] ?? [];
  let seen = -1;
  let idx = list.findIndex((item) => {
    if (item.kind !== "tool_use") return false;
    seen += 1;
    return seen === index;
  });
  if (idx === -1) {
    idx = findLastIndex(list, (item) => item.kind === "tool_use");
  }
  if (idx === -1) return {};
  const item = list[idx];
  if (item.kind !== "tool_use") return {};
  const input = typeof item.input === "string" ? item.input + partialJson : partialJson;
  const next = [...list];
  next[idx] = { ...item, input };
  return { managedLogs: { ...state.managedLogs, [agentId]: next } };
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

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function sendWhenAgentReady(send: () => Promise<void>) {
  let lastError: unknown;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      await send();
      return;
    } catch (e) {
      lastError = e;
      if (!String(e).includes("agent not found")) {
        throw e;
      }
      await sleep(250);
    }
  }
  throw lastError;
}

function handleManagedEvent(
  state: AppState,
  agentId: string,
  ev: Record<string, unknown>,
): Partial<AppState> {
  const type = ev.type as string | undefined;

  if (type === "stream_event") {
    const inner = asRecord(ev.event);
    if (inner.type === "content_block_start") {
      const block = asRecord(inner.content_block);
      if (block.type === "text" && typeof block.text === "string" && block.text) {
        return updateLastAssistantStreaming(state, agentId, block.text);
      }
      if (block.type === "tool_use") {
        return upsertToolUse(state, agentId, {
          kind: "tool_use",
          id: String(block.id ?? ""),
          name: String(block.name ?? "tool"),
          input: block.input ?? "",
        });
      }
      return {};
    }

    const delta = asRecord(inner.delta);
    if (delta.type === "text_delta" && typeof delta.text === "string") {
      return updateLastAssistantStreaming(state, agentId, delta.text);
    }
    if (
      delta.type === "input_json_delta" &&
      typeof delta.partial_json === "string" &&
      typeof inner.index === "number"
    ) {
      return updateToolInputDelta(state, agentId, inner.index, delta.partial_json);
    }
    return {};
  }

  if (type === "assistant") {
    const message = asRecord(ev.message);
    const content = asBlockList(message.content);
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
        const p = upsertToolUse(working, agentId, {
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
    const message = asRecord(ev.message);
    const content = message.content;
    const text = contentText(content);
    let patches = appendUserIfMissing(state, agentId, text);
    let working: AppState = { ...state, ...patches } as AppState;
    if (Array.isArray(content)) {
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
    return patches;
  }

  if (type === "result") {
    const subtype = String(ev.subtype ?? "");
    const isError = ev.is_error === true || subtype !== "success";
    let patches = finalizeStreamingAssistant(state, agentId);
    const working = { ...state, ...patches } as AppState;
    const list = working.managedLogs[agentId] ?? [];
    const lastUserIdx = findLastIndex(list, (item) => item.kind === "user");
    const hasAssistantText = list.slice(lastUserIdx + 1).some(
      (item) => item.kind === "assistant" && item.text.trim().length > 0,
    );
    if (isError) {
      patches = mergePatches(
        patches,
        appendItem(working, agentId, {
          kind: "result",
          text:
            typeof ev.result === "string"
              ? ev.result
              : `Turn failed (${subtype || "error"})`,
          is_error: true,
        }),
      );
    } else if (
      !hasAssistantText &&
      typeof ev.result === "string" &&
      ev.result.trim()
    ) {
      patches = mergePatches(
        patches,
        appendItem(working, agentId, {
          kind: "assistant",
          text: ev.result,
        }),
      );
    }
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

  if (type === "system" && typeof ev.subtype === "string") {
    if (ev.subtype === "status" && typeof ev.status === "string") {
      return appendItem(state, agentId, {
        kind: "system",
        text: `Claude status: ${ev.status}`,
      });
    }
    if (ev.subtype !== "init") {
      return appendItem(state, agentId, {
        kind: "system",
        text: `Claude ${ev.subtype}`,
      });
    }
  }

  return {};
}

/** Landmarks already used by real or draft agents — passed to
 *  `pickLandmark` so re-rolling avoids collisions. */
function usedLandmarks(workspace: Workspace | null, drafts: DraftAgent[]): Set<string> {
  const used = new Set<string>();
  for (const a of workspace?.agents ?? []) {
    if (LANDMARK_NAMES.includes(a.name)) used.add(a.name);
  }
  for (const d of drafts) used.add(d.name);
  return used;
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

  drafts: [],
  activeDraftId: null,
  settingsOpen: false,
  leftCollapsed: false,
  rightCollapsed: false,
  leftWidth: 312,
  rightWidth: 320,

  theme: loadString<ThemeMode>("quorum:theme", "dark"),
  accent: loadString<string>("quorum:accent", "copper"),
  density: loadString<Density>("quorum:density", "comfortable"),
  showLandmarks: localStorage.getItem("quorum:showLandmarks") !== "0",
  features: loadFeatures(),
  providerFlags: loadProviderFlags(),
  viewMode: loadString<WorkspaceView>("quorum:viewMode", "custom"),

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
            a.id === e.agent_id
              ? {
                  ...a,
                  repos: a.repos.map((r) =>
                    r.subdir === e.subdir ? { ...r, branch: e.branch } : r,
                  ),
                }
              : a,
          ),
        },
      });
    });

    await onAgentRepoAdded((e) => {
      const ws = get().workspace;
      if (!ws) return;
      set({
        workspace: {
          ...ws,
          agents: ws.agents.map((a) =>
            a.id === e.agent_id ? { ...a, repos: [...a.repos, e.repo] } : a,
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
      set((state) => ({
        workspace: next,
        managedBusy:
          e.status === "error" || e.status === "stopped"
            ? { ...state.managedBusy, [e.agent_id]: false }
            : state.managedBusy,
      }));
    });

    const workspace = await api.getWorkspace();
    set({ workspace });
  },

  selectAgent: (id) => set({ selectedAgentId: id, activeDraftId: null }),

  addWorkspaceRepo: async (path) => {
    set({ busy: true, lastError: null });
    try {
      const ws = await api.addWorkspaceRepo(path);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  removeWorkspaceRepo: async (path) => {
    try {
      const ws = await api.removeWorkspaceRepo(path);
      set({ workspace: ws });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  spawn: async (view, repoPath) => {
    set({ busy: true, lastError: null });
    try {
      const rec = await api.spawnAgent(view, repoPath);
      const fresh = await api.getWorkspace();
      set((state) => {
        const patches: Partial<AppState> = {
          workspace: fresh,
          selectedAgentId: rec.id,
        };
        if (view === "custom") {
          const primaryParent = rec.repos[0]?.parent_branch;
          const parent = primaryParent ? ` from ${primaryParent}` : "";
          const greeting =
            `Worktree ${rec.id} ready${parent}. ` +
            `Claude is waiting — send a message to begin.`;
          patches.managedLogs = {
            ...state.managedLogs,
            [rec.id]: [{ kind: "system", text: greeting }],
          };
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
      set((state) => ({
        lastError: String(e),
        managedBusy: { ...state.managedBusy, [id]: false },
      }));
    }
  },

  switchView: async (id, view) => {
    if (view === "native") {
      clearOutputBuffer(id);
    }
    set((state) => ({
      managedBusy: { ...state.managedBusy, [id]: false },
      switchInFlight: { ...state.switchInFlight, [id]: true },
    }));
    try {
      await api.switchView(id, view);
      localStorage.setItem("quorum:viewMode", view);
      set({ viewMode: view });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set((state) => ({
        switchInFlight: { ...state.switchInFlight, [id]: false },
      }));
    }
  },

  resume: async (id) => {
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

  // ── drafts ─────────────────────────────────────────────────────────────────
  createDraft: (repoPath) => {
    const { workspace, drafts } = get();
    const name = pickLandmark(usedLandmarks(workspace, drafts));
    const draft: DraftAgent = {
      id: `draft-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      repoPath,
      name,
      provider: DEFAULT_PROVIDER_ID,
      base: "main",
    };
    set((s) => ({
      drafts: [...s.drafts, draft],
      activeDraftId: draft.id,
      selectedAgentId: null,
    }));
  },

  updateDraft: (id, patch) =>
    set((s) => ({
      drafts: s.drafts.map((d) => (d.id === id ? { ...d, ...patch } : d)),
    })),

  removeDraft: (id) =>
    set((s) => ({
      drafts: s.drafts.filter((d) => d.id !== id),
      activeDraftId: s.activeDraftId === id ? null : s.activeDraftId,
    })),

  selectDraft: (id) =>
    set({
      activeDraftId: id,
      selectedAgentId: null,
    }),

  rerollDraftName: (id) => {
    const { workspace, drafts } = get();
    const used = usedLandmarks(workspace, drafts);
    // exclude the current name so the reroll always changes
    const current = drafts.find((d) => d.id === id);
    if (current) used.delete(current.name);
    const next = pickLandmark(new Set([...used, current?.name ?? ""]));
    set({
      drafts: drafts.map((d) => (d.id === id ? { ...d, name: next } : d)),
    });
  },

  spawnFromDraft: async (id, text, _provider) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return;
    set({ busy: true, lastError: null });
    try {
      const view = get().viewMode;
      const rec = await api.spawnAgent(view, draft.repoPath);
      const fresh = await api.getWorkspace();
      set((state) => {
        const patches: Partial<AppState> = {
          workspace: fresh,
          selectedAgentId: rec.id,
          drafts: state.drafts.filter((d) => d.id !== id),
          activeDraftId: null,
        };
        if (view === "custom") {
          patches.managedLogs = {
            ...state.managedLogs,
            [rec.id]: [{ kind: "user", text }],
          };
          patches.managedBusy = { ...state.managedBusy, [rec.id]: true };
        }
        return patches;
      });
      if (view === "native") {
        await sendWhenAgentReady(() =>
          api.writeToAgent(rec.id, text.replace(/\r?\n/g, " ") + "\r"),
        );
      } else {
        await sendWhenAgentReady(() => api.sendUserMessage(rec.id, text));
      }
    } catch (e) {
      const selected = get().selectedAgentId;
      set((state) => ({
        lastError: String(e),
        managedBusy: selected
          ? { ...state.managedBusy, [selected]: false }
          : state.managedBusy,
      }));
    } finally {
      set({ busy: false });
    }
  },

  // ── UI ──────────────────────────────────────────────────────────────────────
  toggleSettings: (open) =>
    set((s) => ({ settingsOpen: open ?? !s.settingsOpen })),
  toggleLeft: () => set((s) => ({ leftCollapsed: !s.leftCollapsed })),
  toggleRight: () => set((s) => ({ rightCollapsed: !s.rightCollapsed })),
  setLeftWidth: (w) => set({ leftWidth: w }),
  setRightWidth: (w) => set({ rightWidth: w }),

  // ── appearance ──────────────────────────────────────────────────────────────
  setTheme: (t) => {
    localStorage.setItem("quorum:theme", t);
    set({ theme: t });
  },
  setAccent: (a) => {
    localStorage.setItem("quorum:accent", a);
    set({ accent: a });
  },
  setDensity: (d) => {
    localStorage.setItem("quorum:density", d);
    set({ density: d });
  },
  setShowLandmarks: (v) => {
    localStorage.setItem("quorum:showLandmarks", v ? "1" : "0");
    set({ showLandmarks: v });
  },
  setFeature: (k, v) =>
    set((s) => {
      const next = { ...s.features, [k]: v };
      localStorage.setItem(
        "quorum:features",
        JSON.stringify(
          Object.fromEntries(FEATURE_KEYS.map((key) => [key, next[key]])),
        ),
      );
      return { features: next };
    }),
  setProviderEnabled: (id, enabled) =>
    set((s) => {
      const next = { ...s.providerFlags, [id]: enabled };
      localStorage.setItem("quorum:providers", JSON.stringify(next));
      return { providerFlags: next };
    }),
  setViewMode: (v) => {
    localStorage.setItem("quorum:viewMode", v);
    set({ viewMode: v });
  },
}));
