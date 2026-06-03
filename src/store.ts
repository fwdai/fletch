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
  onPrStateChanged,
  onShellOutput,
  onWorkspaceChanged,
  type AgentRecord,
  type AgentView,
  type GitState,
  type PrState,
  type ShortStats,
  type Workspace,
} from "./api";
import { DEFAULT_PROVIDER_ID } from "./data/providers";
import { commandsFor } from "./data/slashCommands";
import { getAdapter, type ChatItem, type RawEvent } from "./adapters";
import { getAllSettings, setSetting } from "./storage/settings";
import { deleteMessages, insertMessage } from "./storage/messages";
import {
  getOrCreateAccount,
  saveAccountProfile,
  toProfile,
  type AccountProfile,
} from "./storage/accounts";

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

// ---- Per-agent shell PTY output buffer ----------------------------------
// Mirrors the agent output buffer. Used by TermPanel to repaint after
// tab switch.
const shellSinks = new Map<string, OutputHandler>();
const shellBuffers = new Map<string, Uint8Array>();

export function getShellBuffer(agentId: string): Uint8Array | undefined {
  return shellBuffers.get(agentId);
}

export function clearShellBuffer(agentId: string) {
  shellBuffers.delete(agentId);
}

export function registerShellSink(
  agentId: string,
  handler: OutputHandler,
): () => void {
  shellSinks.set(agentId, handler);
  return () => {
    if (shellSinks.get(agentId) === handler) shellSinks.delete(agentId);
  };
}

// ---- Re-export the normalized ChatItem so component files don't need to
//      import from "./adapters" directly; managedLogs is typed in terms
//      of this. -----------------------------------------------------------
export type { ChatItem } from "./adapters";

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
export type SettingsSection = "general" | "account" | "providers";

export interface FeatureFlags {
  git: boolean;
  files: boolean;
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
  files: true,
  diff: false,
  run: false,
  terminal: false,
  thinkingBudget: true,
  autoEdit: false,
  statusBar: false,
  tokenUsage: false,
};

function parseFeatures(raw: string | undefined): FeatureFlags {
  if (!raw) return DEFAULT_FEATURES;
  try {
    return { ...DEFAULT_FEATURES, ...(JSON.parse(raw) as Partial<FeatureFlags>) };
  } catch {
    return DEFAULT_FEATURES;
  }
}

function parseProviderFlags(raw: string | undefined): Record<string, boolean> {
  if (!raw) return {};
  try {
    return JSON.parse(raw) as Record<string, boolean>;
  } catch {
    return {};
  }
}

interface AppState {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  managedLogs: Record<string, ChatItem[]>;
  /** True while an on-disk Claude transcript is being replayed into
   *  the custom-view log. */
  transcriptLoading: Record<string, boolean>;
  /** True once the current process has attempted transcript replay for
   *  an agent. Prevents repeated reloads when a session has no JSONL. */
  transcriptLoaded: Record<string, boolean>;
  /** True between user sending a turn and claude's `result` event for
   *  that turn. Drives the send-button disabled state and the
   *  "thinking…" indicator. */
  managedBusy: Record<string, boolean>;
  /** Optional label shown alongside the busy indicator, e.g. "Compacting"
   *  for `/compact`. Cleared when the turn ends. */
  managedBusyLabel: Record<string, string | undefined>;
  /** True while a view switch is in flight — disable toggle UI. */
  switchInFlight: Record<string, boolean>;
  /** Last observed input-token count from the agent's most recent
   *  `result` event. Persists across agents so the right-rail
   *  cost panel can show a stable number after a turn completes. */
  tokens: Record<string, number>;
  /** Full git state, keyed by agent_id — branch, ahead/behind, file list,
   *  totals. Only populated for the focused agent (by GitPanel's 1s poll
   *  while it's mounted). For sidebar shortstats / right-rail badges of
   *  other agents, read from `gitShortstats` instead. */
  gitStates: Record<string, GitState>;
  /** Compact per-agent shortstats (additions / deletions / file count),
   *  keyed by agent_id. Updated for every live agent on the app-wide 5s
   *  poll — kept in its own map so the focused agent's richer `gitStates`
   *  entry isn't clobbered by a slower bulk reply. */
  gitShortstats: Record<string, ShortStats>;
  /** Fetch full git state for one agent (used by the focused panel's poll). */
  fetchGitState: (agentId: string) => Promise<void>;
  /** Fetch compact shortstats for every live agent in one round-trip
   *  (used by the app-wide background poll). */
  fetchAllShortstats: () => Promise<void>;
  /** PR state per agent, keyed by agent_id. Updated by the pr:state_changed watcher event. */
  prStates: Record<string, PrState | null>;
  fetchPrState: (agentId: string) => Promise<void>;
  /** Resolves to "up-to-date" | "pushed" on success, null on error. */
  pushAgent: (agentId: string) => Promise<string | null>;
  /** Resolves true on success, false on error. */
  pullAgent: (agentId: string) => Promise<boolean>;
  /** Resolves true on success, false on error. */
  rebaseAgent: (agentId: string) => Promise<boolean>;
  commitChanges: (agentId: string, message: string) => Promise<boolean>;
  /** Commit all changes, push, and open a PR — the "Commit & open PR"
   *  primary CTA wired from the git panel. Returns false on any step
   *  failure so the UI can leave the textarea content in place. */
  commitAndOpenPr: (agentId: string, message: string) => Promise<boolean>;
  stashChanges: (agentId: string) => Promise<void>;
  discardChanges: (agentId: string) => Promise<void>;
  abortMerge: (agentId: string) => Promise<void>;
  deleteBranch: (agentId: string) => Promise<void>;
  createPr: (agentId: string, title: string, body: string) => Promise<PrState | null>;
  mergePr: (agentId: string) => Promise<void>;

  drafts: DraftAgent[];
  activeDraftId: string | null;
  /** Quick-settings popover (gear / ⌘,). */
  settingsOpen: boolean;
  /** Dedicated full-screen settings surface (General / Account / Providers).
   *  Replaces the workspace panes while open. */
  settingsScreenOpen: boolean;
  settingsSection: SettingsSection;
  /** Local account profile, loaded on init. `null` until the row is read. */
  account: AccountProfile | null;
  /** When true the workspace pane shows archived-session history instead
   *  of the selected agent / draft. Treated as a separate "mode" that wins
   *  over `selectedAgentId` / `activeDraftId` for rendering. */
  historyOpen: boolean;
  /** When in history mode, the archived agent whose chat preview is
   *  being shown. `null` = list view. */
  selectedHistoryAgentId: string | null;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftWidth: number;
  rightWidth: number;

  // ── appearance & feature flags ────────────────────────────────────────────
  theme: ThemeMode;
  /** Syntax-highlighting theme for the File panel editor. "quorum" = the
   *  built-in palette; other ids map to a highlight.js theme family that
   *  follows the app's light/dark mode. See data/codeThemes.ts. */
  codeTheme: string;
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
  sendUserMessage: (
    id: string,
    text: string,
    attachments?: string[],
  ) => Promise<void>;
  switchView: (id: string, view: AgentView) => Promise<void>;
  resume: (id: string) => Promise<void>;
  stop: (id: string) => Promise<void>;
  discard: (id: string) => Promise<void>;
  archive: (id: string) => Promise<void>;
  restore: (id: string) => Promise<void>;
  /** Read the on-disk JSONL for an agent and replay it through the
   *  same adapter that processes live events. */
  loadHistoryTranscript: (id: string) => Promise<void>;
  clearError: () => void;

  // drafts
  createDraft: (repoPath: string) => Promise<void>;
  updateDraft: (id: string, patch: Partial<DraftAgent>) => void;
  removeDraft: (id: string) => void;
  selectDraft: (id: string | null) => void;
  rerollDraftName: (id: string) => Promise<void>;
  /** Spawn the real agent for a draft and dispatch the first message. */
  spawnFromDraft: (
    id: string,
    text: string,
    provider: string,
    attachments?: string[],
  ) => Promise<void>;

  // UI
  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  saveAccount: (
    patch: Pick<AccountProfile, "firstName" | "lastName" | "email">,
  ) => Promise<void>;
  toggleHistory: (open?: boolean) => void;
  selectHistoryAgent: (id: string | null) => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;

  // appearance
  setTheme: (t: ThemeMode) => void;
  setCodeTheme: (id: string) => void;
  setAccent: (a: string) => void;
  setDensity: (d: Density) => void;
  setShowLandmarks: (v: boolean) => void;
  setFeature: <K extends keyof FeatureFlags>(k: K, v: FeatureFlags[K]) => void;
  setProviderEnabled: (id: string, enabled: boolean) => void;
  setViewMode: (v: WorkspaceView) => void;
}

function providerFor(state: AppState, agentId: string): string | undefined {
  return state.workspace?.agents.find((a) => a.id === agentId)?.provider;
}

// Labels shown alongside the busy spinner when a known slash command is
// dispatched. The key is the bare command name (no leading slash). Any
// command not listed falls back to the generic "thinking" indicator.
const SLASH_BUSY_LABELS: Record<string, string> = {
  compact: "Compacting",
  init: "Initializing",
  help: "Helping",
};

/** If `text` is a `/<name>` matching a known passthrough command for the
 *  given provider, return its bare name; otherwise null. The result is
 *  used both to swap the optimistic user_message for a slash_command
 *  notice and to set a busy label. */
function passthroughSlashName(
  providerId: string | undefined,
  text: string,
): string | null {
  if (!providerId || !text.startsWith("/")) return null;
  const first = text.split(/\s/)[0].slice(1);
  const match = commandsFor(providerId).find(
    (c) => c.kind === "passthrough" && c.name === first,
  );
  return match ? match.name : null;
}

/** Apply one raw event to an agent's log via its provider adapter. Catches
 *  adapter throws so a single malformed event can't poison the whole log. */
function applyEvent(
  state: AppState,
  agentId: string,
  rawEvent: RawEvent,
): Partial<AppState> {
  const adapter = getAdapter(providerFor(state, agentId));
  const prev = state.managedLogs[agentId] ?? [];
  let next: ChatItem[];
  try {
    next = adapter.reduce(prev, rawEvent);
  } catch (err) {
    console.error("[adapters] reduce threw", {
      provider: adapter.id,
      type: rawEvent.type,
      err,
    });
    return {};
  }
  if (next === prev) return {};

  // `result` events signal turn end for claude; mirror that state on the
  // store so the composer re-enables. Adapter-agnostic: any notice with
  // subtype "turn_end" appended this tick clears managedBusy.
  const turnEnded =
    next.length > prev.length &&
    next[next.length - 1]?.kind === "notice" &&
    (next[next.length - 1] as { subtype?: string }).subtype === "turn_end";

  const tokens = extractInputTokens(rawEvent);

  return {
    managedLogs: { ...state.managedLogs, [agentId]: next },
    managedBusy: turnEnded
      ? { ...state.managedBusy, [agentId]: false }
      : state.managedBusy,
    managedBusyLabel: turnEnded
      ? { ...state.managedBusyLabel, [agentId]: undefined }
      : state.managedBusyLabel,
    tokens:
      tokens !== undefined
        ? { ...state.tokens, [agentId]: tokens }
        : state.tokens,
  };
}

function extractInputTokens(ev: RawEvent): number | undefined {
  if (ev.type !== "result") return undefined;
  const usage = ev.usage as Record<string, unknown> | undefined;
  const n = usage?.input_tokens;
  return typeof n === "number" && n > 0 ? n : undefined;
}

/** Capture the full conversation from Claude's session JSONL into the
 *  messages table. Replaces any previously captured messages for this
 *  agent. Fire-and-forget — errors are logged, not surfaced. */
async function captureTranscript(agentId: string, provider?: string) {
  try {
    const rawLines = await api.readSessionTranscript(agentId);
    if (rawLines.length === 0) return;

    const adapter = getAdapter(provider);
    const events = adapter.normalizeTranscript(rawLines);
    let items: ChatItem[] = [];
    for (const ev of events) {
      items = adapter.reduce(items, ev);
    }
    if (items.length === 0) return;

    await deleteMessages(agentId);
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      const content =
        item.kind === "user_message" || item.kind === "agent_message"
          ? item.text
          : item.kind === "notice"
            ? item.text
            : JSON.stringify(
                "input" in item ? item.input : "content" in item ? item.content : null,
              );
      await insertMessage({
        agent_id: agentId,
        kind: item.kind,
        content: content || "",
        metadata_json:
          item.kind === "tool_call"
            ? JSON.stringify({ name: item.name, id: item.id })
            : item.kind === "tool_result"
              ? JSON.stringify({ tool_use_id: item.tool_use_id, is_error: item.is_error })
              : item.kind === "notice"
                ? JSON.stringify({ subtype: item.subtype })
                : null,
        sequence: i,
      });
    }
  } catch (e) {
    console.warn("[captureTranscript] failed for", agentId, e);
  }
}

/** Names already taken by real or draft agents — passed to the backend
 *  name allocator so picks avoid collisions. */
function usedNames(workspace: Workspace | null, drafts: DraftAgent[]): Set<string> {
  const used = new Set<string>();
  for (const a of workspace?.agents ?? []) used.add(a.name);
  for (const d of drafts) used.add(d.name);
  return used;
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

export const useAppStore = create<AppState>((set, get) => ({
  workspace: null,
  selectedAgentId: null,
  busy: false,
  lastError: null,
  initialized: false,
  managedLogs: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  managedBusyLabel: {},
  switchInFlight: {},
  tokens: {},
  gitStates: {},
  gitShortstats: {},
  prStates: {},

  drafts: [],
  activeDraftId: null,
  settingsOpen: false,
  settingsScreenOpen: false,
  settingsSection: "general" as SettingsSection,
  account: null,
  historyOpen: false,
  selectedHistoryAgentId: null,
  leftCollapsed: false,
  rightCollapsed: false,
  leftWidth: 312,
  rightWidth: 520,

  theme: "dark" as ThemeMode,
  codeTheme: "quorum",
  accent: "copper",
  density: "comfortable" as Density,
  showLandmarks: true,
  features: DEFAULT_FEATURES,
  providerFlags: {},
  viewMode: "custom" as WorkspaceView,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });

    // Load persisted settings from DB.
    try {
      const s = await getAllSettings();
      set({
        theme: (s.theme as ThemeMode) || "dark",
        codeTheme: s.codeTheme || "quorum",
        accent: s.accent || "copper",
        density: (s.density as Density) || "comfortable",
        showLandmarks: s.showLandmarks !== "false",
        features: parseFeatures(s.features),
        providerFlags: parseProviderFlags(s.providers),
        viewMode: (s.viewMode as WorkspaceView) || "custom",
      });
    } catch {
      // First launch or DB not ready — defaults are fine.
    }

    // Load (or lazily create) the single local account profile.
    try {
      const row = await getOrCreateAccount();
      set({ account: toProfile(row) });
    } catch {
      // Non-fatal — Account screen shows empty fields until a save succeeds.
    }

    await onAgentOutput((e) => {
      const chunk = new Uint8Array(e.bytes);
      appendToBuffer(e.agent_id, chunk);
      const sink = outputSinks.get(e.agent_id);
      if (sink) sink(chunk);
    });

    await onShellOutput((e) => {
      const chunk = new Uint8Array(e.bytes);
      const existing = shellBuffers.get(e.agent_id);
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
      shellBuffers.set(e.agent_id, next);
      const sink = shellSinks.get(e.agent_id);
      if (sink) sink(chunk);
    });

    await onAgentEvent((e) => {
      set((state) => applyEvent(state, e.agent_id, e.event as RawEvent));
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
        managedLogs:
          e.status === "stopped" && (state.managedBusy[e.agent_id] ?? false)
            ? {
                ...state.managedLogs,
                [e.agent_id]: [
                  ...(state.managedLogs[e.agent_id] ?? []),
                  {
                    kind: "notice",
                    subtype: "info",
                    text: "Agent was interrupted.",
                  },
                ],
              }
            : state.managedLogs,
        managedBusy:
          e.status === "error" || e.status === "stopped" || e.status === "idle"
            ? { ...state.managedBusy, [e.agent_id]: false }
            : state.managedBusy,
      }));
    });

    // Archive / restore reshape `repos` and `archive` on the record,
    // which `agent:status` alone doesn't cover. The backend emits this
    // small ping after either operation; we reload the workspace.
    await onWorkspaceChanged(async () => {
      const fresh = await api.getWorkspace();
      if (fresh) set({ workspace: fresh });
    });

    await onPrStateChanged((e) => {
      set((s) => ({ prStates: { ...s.prStates, [e.agent_id]: e.state } }));
    });

    const workspace = await api.getWorkspace();
    set({ workspace });
  },

  selectAgent: (id) =>
    set({
      selectedAgentId: id,
      activeDraftId: null,
      historyOpen: false,
      selectedHistoryAgentId: null,
    }),

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
          patches.managedLogs = { ...state.managedLogs, [rec.id]: [] };
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

  sendUserMessage: async (id, text, attachments = []) => {
    try {
      set((state) => {
        const slashName = passthroughSlashName(providerFor(state, id), text);
        const entry: ChatItem = slashName
          ? { kind: "notice", subtype: "slash_command", text: `/${slashName}` }
          : { kind: "user_message", text };
        return {
          managedLogs: {
            ...state.managedLogs,
            [id]: [...(state.managedLogs[id] ?? []), entry],
          },
          managedBusy: { ...state.managedBusy, [id]: true },
          managedBusyLabel: {
            ...state.managedBusyLabel,
            [id]: slashName ? SLASH_BUSY_LABELS[slashName] : undefined,
          },
        };
      });
      try {
        await api.sendUserMessage(id, text, attachments);
      } catch (e) {
        if (String(e).includes("agent not found")) {
          // Dead idle agent (finished its prior task) — resume the
          // process in --resume mode, then deliver the message once ready.
          await api.resumeAgent(id);
          await sendWhenAgentReady(() =>
            api.sendUserMessage(id, text, attachments),
          );
        } else {
          throw e;
        }
      }
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
      if (view === "custom") {
        await get().loadHistoryTranscript(id);
      }
      set({ viewMode: view });
      setSetting("viewMode", view);
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
        const { [id]: _droppedTranscriptLoading, ...restTranscriptLoading } =
          s.transcriptLoading;
        const { [id]: _droppedTranscriptLoaded, ...restTranscriptLoaded } =
          s.transcriptLoaded;
        const { [id]: _droppedBusy, ...restBusy } = s.managedBusy;
        const { [id]: _droppedTokens, ...restTokens } = s.tokens;
        const { [id]: _droppedGitState, ...restGitStates } = s.gitStates;
        const { [id]: _droppedShortstats, ...restShortstats } = s.gitShortstats;
        const { [id]: _droppedPrState, ...restPrStates } = s.prStates;
        return {
          workspace: fresh,
          selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
          managedLogs: restLogs,
          transcriptLoading: restTranscriptLoading,
          transcriptLoaded: restTranscriptLoaded,
          managedBusy: restBusy,
          tokens: restTokens,
          gitStates: restGitStates,
          gitShortstats: restShortstats,
          prStates: restPrStates,
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  archive: async (id) => {
    try {
      const provider = providerFor(get(), id);
      await captureTranscript(id, provider);
      await api.archiveAgent(id);
      clearOutputBuffer(id);
      const fresh = await api.getWorkspace();
      set((s) => {
        // Drop ephemeral state for the agent — the user can re-open
        // the transcript through History, which loads it fresh from disk.
        const { [id]: _l, ...restLogs } = s.managedLogs;
        const { [id]: _tl, ...restTranscriptLoading } = s.transcriptLoading;
        const { [id]: _td, ...restTranscriptLoaded } = s.transcriptLoaded;
        const { [id]: _b, ...restBusy } = s.managedBusy;
        const { [id]: _t, ...restTokens } = s.tokens;
        const { [id]: _g, ...restGitStates } = s.gitStates;
        const { [id]: _s, ...restShortstats } = s.gitShortstats;
        const { [id]: _p, ...restPrStates } = s.prStates;
        return {
          workspace: fresh ?? s.workspace,
          selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
          managedLogs: restLogs,
          transcriptLoading: restTranscriptLoading,
          transcriptLoaded: restTranscriptLoaded,
          managedBusy: restBusy,
          tokens: restTokens,
          gitStates: restGitStates,
          gitShortstats: restShortstats,
          prStates: restPrStates,
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  restore: async (id) => {
    try {
      await api.restoreAgent(id);
      const fresh = await api.getWorkspace();
      // Keep the JSONL-replayed log in place — claude's `--resume` in
      // stream-json mode emits new events on top of the existing
      // conversation, so the chat view picks up exactly where the
      // preview left off.
      set((s) => ({
        workspace: fresh ?? s.workspace,
        historyOpen: false,
        selectedHistoryAgentId: null,
        selectedAgentId: id,
      }));
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  loadHistoryTranscript: async (id) => {
    if (get().transcriptLoading[id]) return;
    set((s) => ({
      transcriptLoading: { ...s.transcriptLoading, [id]: true },
    }));
    try {
      const rawLines = await api.readSessionTranscript(id);
      const adapter = getAdapter(providerFor(get(), id));
      const events = adapter.normalizeTranscript(rawLines);
      let items: ChatItem[] = [];
      for (const ev of events) {
        items = adapter.reduce(items, ev);
      }
      set((state) => {
        // If the on-disk JSONL produced nothing but we have an active
        // in-memory turn, leave the log alone — claude hasn't flushed
        // yet and we don't want to erase a turn that's still in flight.
        if (items.length === 0 && (state.managedLogs[id]?.length ?? 0) > 0) {
          return {};
        }
        return {
          managedLogs: { ...state.managedLogs, [id]: items },
          managedBusy: { ...state.managedBusy, [id]: false },
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    } finally {
      set((s) => ({
        transcriptLoading: { ...s.transcriptLoading, [id]: false },
        transcriptLoaded: { ...s.transcriptLoaded, [id]: true },
      }));
    }
  },

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
      // pr:state_changed event will update prStates
      captureTranscript(agentId, providerFor(get(), agentId));
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  clearError: () => set({ lastError: null }),

  // ── drafts ─────────────────────────────────────────────────────────────────
  createDraft: async (repoPath) => {
    const { workspace, drafts } = get();
    const used = [...usedNames(workspace, drafts)];
    const name = await api.allocateDraftName(used);
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

  rerollDraftName: async (id) => {
    const { workspace, drafts } = get();
    const used = usedNames(workspace, drafts);
    // Keep the current name in `used` so the allocator picks a different one.
    const next = await api.allocateDraftName([...used]);
    set((s) => ({
      drafts: s.drafts.map((d) => (d.id === id ? { ...d, name: next } : d)),
    }));
  },

  spawnFromDraft: async (id, text, provider, attachments = []) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return;
    set({ busy: true, lastError: null });
    try {
      const view = get().viewMode;
      const rec = await api.spawnAgent(view, draft.repoPath, provider);
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
            [rec.id]: [{ kind: "user_message", text }],
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
        await sendWhenAgentReady(() =>
          api.sendUserMessage(rec.id, text, attachments),
        );
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
  openSettingsScreen: (section) =>
    set((s) => ({
      settingsScreenOpen: true,
      settingsSection: section ?? s.settingsSection,
      // The full screen takes over — dismiss the quick popover behind it.
      settingsOpen: false,
    })),
  closeSettingsScreen: () => set({ settingsScreenOpen: false }),
  setSettingsSection: (section) => set({ settingsSection: section }),
  saveAccount: async (patch) => {
    const current = get().account;
    if (!current) return;
    try {
      await saveAccountProfile(current.id, patch);
      set({ account: { ...current, ...patch } });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  toggleHistory: (open) =>
    set((s) => {
      const next = open ?? !s.historyOpen;
      // Closing history clears any in-flight detail selection so the
      // next open lands on the list.
      return next
        ? { historyOpen: true }
        : { historyOpen: false, selectedHistoryAgentId: null };
    }),
  selectHistoryAgent: (id) => set({ selectedHistoryAgentId: id }),
  toggleLeft: () => set((s) => ({ leftCollapsed: !s.leftCollapsed })),
  toggleRight: () => set((s) => ({ rightCollapsed: !s.rightCollapsed })),
  setLeftWidth: (w) => set({ leftWidth: w }),
  setRightWidth: (w) => set({ rightWidth: w }),

  // ── appearance ──────────────────────────────────────────────────────────────
  setTheme: (t) => {
    set({ theme: t });
    setSetting("theme", t);
  },
  setCodeTheme: (id) => {
    set({ codeTheme: id });
    setSetting("codeTheme", id);
  },
  setAccent: (a) => {
    set({ accent: a });
    setSetting("accent", a);
  },
  setDensity: (d) => {
    set({ density: d });
    setSetting("density", d);
  },
  setShowLandmarks: (v) => {
    set({ showLandmarks: v });
    setSetting("showLandmarks", String(v));
  },
  setFeature: (k, v) =>
    set((s) => {
      const next = { ...s.features, [k]: v };
      setSetting("features", next);
      return { features: next };
    }),
  setProviderEnabled: (id, enabled) =>
    set((s) => {
      const next = { ...s.providerFlags, [id]: enabled };
      setSetting("providers", next);
      return { providerFlags: next };
    }),
  setViewMode: (v) => {
    set({ viewMode: v });
    setSetting("viewMode", v);
  },
}));
