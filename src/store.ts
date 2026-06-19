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
  onSessionRecordsAppended,
  onShellOutput,
  onWorkspaceChanged,
  type AgentRecord,
  type AgentView,
  type GitState,
  type PrChecks,
  type PrComments,
  type PrState,
  type SessionRecord,
  type ShortStats,
  type UserTurn,
  type Workspace,
} from "./api";
import { DEFAULT_PROVIDER_ID } from "./data/providers";
import type {
  GitDelegation,
  GitDelegationKind,
} from "./components/RightPanel/delegation";
import {
  isCommitAction,
  type GitCommitAction,
} from "./components/RightPanel/primaryActions";
import { commandsFor } from "./data/slashCommands";
import { getAdapter, type ChatItem, type RawEvent } from "./adapters";
import { usageFromRecords, hasUsage, type AgentUsage } from "./adapters/usage";
import {
  loadCachedCatalog,
  loadPackagedCatalog,
  refreshCatalog,
  type SlimCatalog,
} from "./data/modelCatalog";
import { getAllSettings, setSetting } from "./storage/settings";
import {
  getAccount,
  getOrCreateAccount,
  saveAccountProfile,
  toProfile,
  type AccountProfile,
} from "./storage/accounts";
import { playAgentDone } from "./util/sound";

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

// Agents the user just stopped. A killed turn may still flush a final `result`
// event (→ turn_end) as it dies; this set suppresses the completion chime for
// that one turn_end so a manual stop doesn't sound like a successful finish.
const interruptedAgents = new Set<string>();

export function getShellBuffer(agentId: string): Uint8Array | undefined {
  return shellBuffers.get(agentId);
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
export type { AgentUsage } from "./adapters/usage";

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
export type SettingsSection = "general" | "account" | "providers" | "developer";

export interface FeatureFlags {
  git: boolean;
  /** The unified Code panel: file explorer/editor + the Live diff feed. */
  code: boolean;
  run: boolean;
  terminal: boolean;
  thinkingBudget: boolean;
  autoEdit: boolean;
  statusBar: boolean;
  tokenUsage: boolean;
}

const DEFAULT_FEATURES: FeatureFlags = {
  git: true,
  code: true,
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
    const saved = JSON.parse(raw) as Partial<FeatureFlags> & {
      // legacy flags folded into `code`
      files?: boolean;
      diff?: boolean;
    };
    // The old "Files" and "Diff" tabs were merged into the Code panel; honor a
    // saved preference for either when migrating an existing settings blob.
    const legacyCode =
      saved.code ?? (saved.files !== undefined || saved.diff !== undefined
        ? !!(saved.files || saved.diff)
        : undefined);
    const { files: _files, diff: _diff, ...rest } = saved;
    void _files;
    void _diff;
    return {
      ...DEFAULT_FEATURES,
      ...rest,
      ...(legacyCode !== undefined ? { code: legacyCode } : {}),
    };
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
  /** Question tools the agent is paused on, awaiting a human answer.
   *  Keyed by agent id, then by the tool_use id of the held `AskUserQuestion`
   *  call → the control-protocol `request_id` to answer it with. Populated when
   *  the backend forwards a held `can_use_tool` request; cleared on answer or
   *  turn end. The widget uses it to know a question is answerable and to route
   *  the answer back as the tool result (the real pause). */
  pendingToolUse: Record<string, Record<string, string>>;
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
  /** True for agents that completed a turn while not focused — drives the
   *  "new results to review" dot in the sidebar. Set on turn-end for any
   *  non-selected agent (covers research-only turns with no diff), cleared
   *  when the agent is selected. */
  unseenResults: Record<string, boolean>;
  /** Per-agent cumulative token usage (and latest context-window fill),
   *  folded from session_records at turn-end and on transcript load. Keyed by
   *  agent_id; absent until the agent's first turn lands. Empty for agents that
   *  don't persist usage on disk (cursor, antigravity). See adapters/usage.ts. */
  usage: Record<string, AgentUsage>;
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
  /** Rich PR merge-gate + checks per agent. Absent key = not yet fetched;
   *  `null` = confirmed unavailable (no PR / gh failure). */
  prChecks: Record<string, PrChecks | null>;
  fetchPrChecks: (agentId: string) => Promise<void>;
  /** Unresolved PR review comments per agent. Absent = not yet fetched;
   *  `null` = confirmed unavailable (no PR / gh failure). */
  prComments: Record<string, PrComments | null>;
  fetchPrComments: (agentId: string) => Promise<void>;
  /** Pending text to push into an agent's chat composer (the "→ chat" quick
   *  action on a review comment). Generic, single-channel: a new seed for an
   *  agent appends to any unconsumed one. The Composer applies and clears it. */
  composerSeeds: Record<string, string>;
  seedComposer: (agentId: string, text: string) => void;
  consumeComposerSeed: (agentId: string) => void;
  /** Active agent-delegated git action per agent (absent = none). Set when a
   *  panel action hands control to the agent; cleared by the panel when the
   *  watched git/PR transition lands or the agent gives up. */
  gitDelegations: Record<string, GitDelegation>;
  delegateGitAction: (agentId: string, kind: GitDelegationKind, prompt: string) => void;
  markGitDelegationRunning: (agentId: string) => void;
  /** The pre-existing turn the delegation was queued behind has settled —
   *  drop `queued` and restart the give-up clock for our own turn. */
  markGitDelegationDequeued: (agentId: string) => void;
  clearGitDelegation: (agentId: string) => void;
  /** Sticky changes-state commit mode (Commit / & push / & open PR). Global
   *  across workspaces, persisted in settings until the user picks another. */
  gitCommitAction: GitCommitAction;
  setGitCommitAction: (action: GitCommitAction) => void;
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
  /** First-run onboarding overlay. `onboardingComplete` is persisted (DB
   *  settings); the overlay auto-opens for new users on init and is
   *  re-openable any time from Settings › General. */
  onboardingOpen: boolean;
  onboardingComplete: boolean;
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
  /** Live-probed version strings keyed by provider id. Populated async on
   *  init; falls back to hardcoded defaults in PROVIDERS when missing. */
  providerVersions: Record<string, string>;
  /** Resolved binary paths keyed by provider id, from the version probe. */
  providerPaths: Record<string, string>;
  /** Per-model metadata (context window, reasoning support) keyed by bare model
   *  id, sourced from models.dev. Initialized synchronously from the cached or
   *  bundled snapshot, then refreshed from the network on init. */
  modelCatalog: SlimCatalog;
  /** View mode preference for the workspace pane. Persisted; falls
   *  back to the agent's own `view` field for native vs. custom
   *  switching. */
  viewMode: WorkspaceView;

  // ── actions ────────────────────────────────────────────────────────────────
  init: () => Promise<void>;
  selectAgent: (id: string | null) => void;
  addWorkspaceRepo: (path: string) => Promise<void>;
  removeWorkspaceRepo: (path: string) => Promise<void>;
  // Clone/create resolve on success and throw on failure, so the New Project
  // modal can show the error inline rather than in the global banner.
  cloneRepo: (spec: string, destParent: string) => Promise<void>;
  createRepo: (
    name: string,
    destParent: string,
    isPrivate: boolean,
    description?: string,
  ) => Promise<void>;
  spawn: (view: AgentView, repoPath: string) => Promise<AgentRecord | null>;
  sendUserMessage: (
    id: string,
    text: string,
    attachments?: string[],
    thinking?: string,
  ) => Promise<void>;
  /** Answer a paused user-input tool (Claude's AskUserQuestion/ExitPlanMode).
   *  Looks up the held control-protocol request for `toolUseId` and delivers
   *  `updatedInput` (the tool's input with the user's `answers` merged in) as
   *  an allow/deny control response, resuming the turn. No-op if no held request
   *  matches (e.g. replayed history, where the answer routes as a normal
   *  message instead). */
  answerToolUse: (
    id: string,
    toolUseId: string,
    updatedInput: unknown,
    behavior?: "allow" | "deny",
    message?: string,
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

  /** Version string of an update that's been downloaded + staged and is
   *  waiting for a restart to take effect. `null` = none pending. */
  updateReadyVersion: string | null;
  /** Record that an update has been staged (drives the restart toast). */
  setUpdateReady: (version: string) => void;
  /** Dismiss the restart toast. The staged update still applies on next launch. */
  dismissUpdate: () => void;

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
    thinking?: string,
  ) => Promise<void>;

  // UI
  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Open the onboarding overlay (e.g. "Replay tour" from Settings). */
  openOnboarding: () => void;
  /** Dismiss onboarding and mark it complete so it won't auto-open again. */
  closeOnboarding: () => void;
  saveAccount: (
    patch: Pick<AccountProfile, "firstName" | "lastName" | "email">,
  ) => Promise<void>;
  /** Re-read the local account row into the store — e.g. after an OAuth
   *  sign-in writes the provider profile to SQLite. */
  refreshAccount: () => Promise<void>;
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
  /** Re-probe installed provider CLIs for versions + binary paths. Runs once
   *  on init and again when the user re-scans from the Providers settings. */
  refreshProviderVersions: () => Promise<void>;
  /** Pull the latest model metadata from models.dev and update state + cache.
   *  Runs once on init; non-fatal on failure (keeps cached/bundled data). */
  refreshModelCatalog: () => Promise<void>;
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

/** Render canonical `session_records` (verbatim per-provider transcript
 *  bodies) into chat items via the same pipeline as on-disk replay:
 *  `normalizeTranscript` → `reduce`. Defensive: a malformed body or an adapter
 *  throw degrades gracefully instead of failing the whole restore. */
export function reduceRecords(
  provider: string | undefined,
  records: SessionRecord[],
): ChatItem[] {
  const adapter = getAdapter(provider);
  let rawEvents: RawEvent[];
  try {
    rawEvents = adapter.normalizeTranscript(records.map((r) => r.body));
  } catch (err) {
    console.error("[adapters] normalizeTranscript threw during restore", {
      provider,
      err,
    });
    return [];
  }
  let items: ChatItem[] = [];
  for (const ev of rawEvents) {
    try {
      items = adapter.reduce(items, ev);
    } catch (err) {
      console.error("[adapters] reduce threw during records restore", {
        provider,
        type: ev.type,
        err,
      });
    }
  }
  return items;
}

/** Overlay Quorum-origin outgoing-turn metadata (attachments) onto the
 *  transcript-rendered conversation. Additive only — never replaces transcript
 *  content (which stays the canonical, re-ingestable history):
 *  - Matched turns (`native_id` set) hang their attachments on the rendered
 *    user message. Aligned from the end, so older turns that predate this
 *    feature (no row) simply keep no attachments instead of mis-grabbing them.
 *  - Pending turns (`native_id` null — the agent never logged them, e.g. a
 *    failed send) render standalone so the message survives reload + retry. */
export function applyUserTurns(items: ChatItem[], turns: UserTurn[]): ChatItem[] {
  if (turns.length === 0) return items;

  const matched = turns.filter((t) => t.native_id);
  const pending = turns.filter((t) => !t.native_id);
  const result = items.map((it) => ({ ...it }));

  const userIdxs: number[] = [];
  result.forEach((it, i) => {
    if (it.kind === "user_message") userIdxs.push(i);
  });

  // End-align matched turns to the trailing rendered user messages.
  const n = Math.min(matched.length, userIdxs.length);
  for (let k = 1; k <= n; k++) {
    const t = matched[matched.length - k];
    const item = result[userIdxs[userIdxs.length - k]];
    if (item.kind === "user_message" && t.attachments.length > 0) {
      item.attachments = t.attachments;
      // Render the clean text the user actually typed (what the live render
      // showed) rather than the transcript's copy, which the runner padded
      // with `Attached file: <path>` reference lines. The stored turn text is
      // verbatim what was sent, so it matches the optimistic render exactly.
      // Prefix-guard so a mis-aligned match can't rewrite an unrelated message.
      if (item.text.startsWith(t.text)) {
        item.text = t.text;
      }
    }
  }

  for (const t of pending) {
    const item: ChatItem = { kind: "user_message", text: t.text };
    if (t.attachments.length > 0) item.attachments = t.attachments;
    result.push(item);
  }

  return result;
}

/** Apply one raw event to an agent's log via its provider adapter. Pure: it
 *  returns the state patch plus a `turnEnded` flag so the caller can fire any
 *  side effects (e.g. the completion chime). Catches adapter throws so a single
 *  malformed event can't poison the whole log. */
function applyEvent(
  state: AppState,
  agentId: string,
  rawEvent: RawEvent,
): { patch: Partial<AppState>; turnEnded: boolean } {
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
    return { patch: {}, turnEnded: false };
  }
  if (next === prev) return { patch: {}, turnEnded: false };

  // `result` events signal turn end for claude; mirror that state on the
  // store so the composer re-enables. Adapter-agnostic: any notice with
  // subtype "turn_end" appended this tick clears managedBusy. The `next !== prev`
  // guard above means this is true exactly once per turn-end.
  const turnEnded =
    next.length > prev.length &&
    next[next.length - 1]?.kind === "notice" &&
    (next[next.length - 1] as { subtype?: string }).subtype === "turn_end";

  return {
    turnEnded,
    patch: {
      managedLogs: { ...state.managedLogs, [agentId]: next },
      managedBusy: turnEnded
        ? { ...state.managedBusy, [agentId]: false }
        : state.managedBusy,
      managedBusyLabel: turnEnded
        ? { ...state.managedBusyLabel, [agentId]: undefined }
        : state.managedBusyLabel,
    },
  };
}

/** Cursor reports token usage only on its live `result` event (never on disk),
 *  so persist that event into session_records (`live_compiled`) when it lands —
 *  then usage folds from records like every other agent, surviving restarts.
 *  Idempotent on the event's `request_id`; after persisting, re-fold so the
 *  gauge updates this turn rather than only on the next records refresh. */
async function persistLiveUsage(
  get: () => AppState,
  set: (patch: Partial<AppState>) => void,
  agentId: string,
  rawEvent: RawEvent,
): Promise<void> {
  const provider = providerFor(get(), agentId);
  const adapter = getAdapter(provider);
  if (!adapter.persistLiveUsage || !adapter.extractUsage) return;
  if (!adapter.extractUsage(rawEvent)) return; // nothing to persist this event
  const nativeId =
    typeof rawEvent.request_id === "string" && rawEvent.request_id
      ? rawEvent.request_id
      : `usage:${Date.now()}`;
  try {
    await api.appendLiveRecord(agentId, provider ?? adapter.id, nativeId, rawEvent);
    const records = await api.readSessionRecords(agentId);
    const usage = usageFromRecords(provider, records);
    if (hasUsage(usage)) {
      set({ usage: { ...get().usage, [agentId]: usage } });
    }
  } catch {
    // Non-critical: the next records refresh or restart re-folds it.
  }
}

/** A per-turn agent captures its session id on its first turn (e.g. agy reads
 *  it from disk at turn-end), but the id only reaches the live frontend via a
 *  full `getWorkspace`. True when an agent's turn just landed yet its session
 *  id is still missing locally — the cue to re-fetch so the Native toggle
 *  unblocks without a reload. False once present, to avoid per-turn re-fetch. */
export function needsSessionIdRefresh(
  workspace: Workspace | null,
  agentId: string,
): boolean {
  const agent = workspace?.agents.find((a) => a.id === agentId);
  return !!agent && !agent.session_id;
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
  updateReadyVersion: null,
  initialized: false,
  managedLogs: {},
  pendingToolUse: {},
  transcriptLoading: {},
  transcriptLoaded: {},
  managedBusy: {},
  managedBusyLabel: {},
  switchInFlight: {},
  unseenResults: {},
  usage: {},
  gitStates: {},
  gitShortstats: {},
  prStates: {},
  prChecks: {},
  prComments: {},
  composerSeeds: {},
  gitDelegations: {},
  gitCommitAction: "agent-commit-pr" as GitCommitAction,

  drafts: [],
  activeDraftId: null,
  settingsOpen: false,
  settingsScreenOpen: false,
  settingsSection: "general" as SettingsSection,
  onboardingOpen: false,
  onboardingComplete: false,
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
  providerVersions: {},
  providerPaths: {},
  modelCatalog: loadCachedCatalog(),
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
        gitCommitAction: isCommitAction(s.gitCommitAction) ? s.gitCommitAction : "agent-commit-pr",
        onboardingComplete: s.onboardingComplete === "true",
        // Auto-open the welcome tour for new users (no completion flag yet).
        onboardingOpen: s.onboardingComplete !== "true",
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

    // Probe installed provider CLIs for real versions + paths (async,
    // non-blocking). Errors are non-fatal — UI falls back to hardcoded versions.
    void get().refreshProviderVersions();

    // Refresh model metadata from models.dev (async, non-blocking). State starts
    // from the cached/bundled snapshot, so lookups work immediately regardless.
    void get().refreshModelCatalog();

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
      const ev = e.event as RawEvent;
      // A held permission prompt the backend forwarded for a human to answer
      // (Claude's AskUserQuestion / ExitPlanMode). Record request_id ↔
      // tool_use_id so the widget can answer it; this is control plane, not a
      // transcript event, so don't feed the reducer. The agent is paused awaiting input — the
      // composer stays disabled (busy) and ChatView hides the "thinking" dots.
      if (ev?.type === "control_request") {
        const req = (ev as { request?: Record<string, unknown> }).request;
        const requestId = (ev as { request_id?: string }).request_id;
        const toolUseId = req?.tool_use_id;
        if (req?.subtype === "can_use_tool" && typeof toolUseId === "string" && requestId) {
          set((state) => ({
            pendingToolUse: {
              ...state.pendingToolUse,
              [e.agent_id]: {
                ...(state.pendingToolUse[e.agent_id] ?? {}),
                [toolUseId]: requestId,
              },
            },
          }));
        }
        return;
      }
      let turnEnded = false;
      set((state) => {
        const result = applyEvent(state, e.agent_id, e.event as RawEvent);
        turnEnded = result.turnEnded;
        return result.patch;
      });
      // Capture usage that lives only on the live stream (cursor) into
      // session_records so it folds like every other agent (see persistLiveUsage).
      void persistLiveUsage(get, set, e.agent_id, e.event as RawEvent);
      // A turn can't end with prompts still held — clear any stale entries
      // (e.g. an interrupt that denied a pending question).
      if (turnEnded && get().pendingToolUse[e.agent_id]) {
        set((state) => ({
          pendingToolUse: { ...state.pendingToolUse, [e.agent_id]: {} },
        }));
      }
      // Side effect lives here, at the call-site, rather than inside the pure
      // updater: chime when an agent turn lands successfully. Skip it if the
      // user stopped this agent — the turn_end is just the killed process
      // flushing its final event, not a real completion.
      if (turnEnded) {
        // `delete` returns true when the agent was interrupted; consume the
        // flag once and gate both the chime and the unseen-results marker on
        // a genuine completion (a manual stop is neither).
        if (!interruptedAgents.delete(e.agent_id)) {
          playAgentDone();
          // Flag results for review on any agent the user isn't currently
          // looking at — this is the only signal for research-only turns that
          // leave no diff behind. Cleared when the agent is selected.
          if (get().selectedAgentId !== e.agent_id) {
            set((state) => ({
              unseenResults: { ...state.unseenResults, [e.agent_id]: true },
            }));
          }
        }
      }
    });

    // A turn's transcript was ingested into session_records: replace the
    // ephemeral live render with the canonical one (richer — e.g. tool results
    // the live stream dropped). No-op if nothing was stored.
    await onSessionRecordsAppended((e) => {
      const id = e.agent_id;
      void (async () => {
        try {
          const [records, turns] = await Promise.all([
            api.readSessionRecords(id),
            api.readUserTurns(id),
          ]);
          if (records.length === 0) return;
          const provider = providerFor(get(), id);
          const items = applyUserTurns(reduceRecords(provider, records), turns);
          const usage = usageFromRecords(provider, records);
          set((state) => ({
            managedLogs: { ...state.managedLogs, [id]: items },
            // Only overwrite when records carried usage — cursor folds usage
            // live, so an empty records result must not wipe it.
            usage: hasUsage(usage) ? { ...state.usage, [id]: usage } : state.usage,
          }));
          // The first turn captures the agent's session id in the DB; pull it
          // into the live workspace so the Native toggle unblocks without a
          // reload. Only when still missing locally — avoids per-turn re-fetch.
          if (needsSessionIdRefresh(get().workspace, id)) {
            const fresh = await api.getWorkspace();
            if (fresh) set({ workspace: fresh });
          }
        } catch {
          // Non-critical refresh; the next load picks up the records.
        }
      })();
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
      // A new turn starting clears any stale stop-suppression flag: if the
      // killed process never flushed a turn_end, this ensures the next genuine
      // completion still chimes.
      if (e.status === "running") interruptedAgents.delete(e.agent_id);
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
        // `running` is the backend's authoritative "a turn is in flight"
        // signal — re-assert busy here so a stale `idle` (e.g. the one
        // start_process emits just before the first turn lands) can't
        // leave the spinner off. `idle`/`error`/`stopped` clear it.
        managedBusy:
          e.status === "running"
            ? { ...state.managedBusy, [e.agent_id]: true }
            : e.status === "error" ||
                e.status === "stopped" ||
                e.status === "idle"
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
    set((state) => {
      // Focusing an agent marks its results as seen — drop the key entirely
      // so the map stays minimal and an absent key is the canonical "seen"
      // state (matching how the component reads it with `?? false`).
      let unseenResults = state.unseenResults;
      if (id && id in unseenResults) {
        const { [id]: _seen, ...rest } = unseenResults;
        unseenResults = rest;
      }
      return {
        selectedAgentId: id,
        activeDraftId: null,
        historyOpen: false,
        selectedHistoryAgentId: null,
        unseenResults,
      };
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

  cloneRepo: async (spec, destParent) => {
    // The new project appears in the sidebar via the refreshed workspace.
    // Errors propagate to the caller (the modal) for inline display.
    const ws = await api.cloneRepo(spec, destParent);
    set({ workspace: ws });
  },

  createRepo: async (name, destParent, isPrivate, description) => {
    const ws = await api.createRepo(name, destParent, isPrivate, description);
    set({ workspace: ws });
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

  sendUserMessage: async (id, text, attachments = [], thinking) => {
    // Stable per-turn id, reused across the agent-not-ready retry below so the
    // backend's session_user_turns write is idempotent (one row per turn).
    const turnId = crypto.randomUUID();
    try {
      set((state) => {
        const slashName = passthroughSlashName(providerFor(state, id), text);
        const entry: ChatItem = slashName
          ? { kind: "notice", subtype: "slash_command", text: `/${slashName}` }
          : attachments.length > 0
            ? { kind: "user_message", text, attachments }
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
        await api.sendUserMessage(id, turnId, text, attachments, thinking);
      } catch (e) {
        if (String(e).includes("agent not found")) {
          // Dead idle agent (finished its prior task) — resume the
          // process in --resume mode, then deliver the message once ready.
          await api.resumeAgent(id);
          await sendWhenAgentReady(() =>
            api.sendUserMessage(id, turnId, text, attachments, thinking),
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

  answerToolUse: async (id, toolUseId, updatedInput, behavior = "allow", message) => {
    const requestId = get().pendingToolUse[id]?.[toolUseId];
    if (!requestId) return;
    // Drop the held prompt and mark busy: feeding the answer resumes the
    // paused turn. The transcript records the resulting tool_result, so there's
    // no separate durable row to write.
    set((state) => {
      const forAgent = { ...(state.pendingToolUse[id] ?? {}) };
      delete forAgent[toolUseId];
      return {
        pendingToolUse: { ...state.pendingToolUse, [id]: forAgent },
        managedBusy: { ...state.managedBusy, [id]: true },
        managedBusyLabel: { ...state.managedBusyLabel, [id]: undefined },
      };
    });
    try {
      await api.answerToolUse(id, requestId, updatedInput, behavior, message);
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
    // Mark this agent as user-stopped so the completion chime is suppressed for
    // any final turn_end the dying process flushes. Set before the await so it
    // lands ahead of any event the backend emits in response.
    interruptedAgents.add(id);
    try {
      await api.stopAgent(id);
    } catch (e) {
      interruptedAgents.delete(id);
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
        const { [id]: _droppedUsage, ...restUsage } = s.usage;
        const { [id]: _droppedGitState, ...restGitStates } = s.gitStates;
        const { [id]: _droppedShortstats, ...restShortstats } = s.gitShortstats;
        const { [id]: _droppedPrState, ...restPrStates } = s.prStates;
        const { [id]: _droppedChecks, ...restPrChecks } = s.prChecks;
        const { [id]: _droppedComments, ...restPrComments } = s.prComments;
        const { [id]: _droppedSeed, ...restComposerSeeds } = s.composerSeeds;
        const { [id]: _droppedDelegation, ...restDelegations } = s.gitDelegations;
        return {
          workspace: fresh,
          selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
          managedLogs: restLogs,
          transcriptLoading: restTranscriptLoading,
          transcriptLoaded: restTranscriptLoaded,
          managedBusy: restBusy,
          usage: restUsage,
          gitStates: restGitStates,
          gitShortstats: restShortstats,
          prStates: restPrStates,
          prChecks: restPrChecks,
          prComments: restPrComments,
          composerSeeds: restComposerSeeds,
          gitDelegations: restDelegations,
        };
      });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  archive: async (id) => {
    try {
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
        const { [id]: _t, ...restUsage } = s.usage;
        const { [id]: _g, ...restGitStates } = s.gitStates;
        const { [id]: _s, ...restShortstats } = s.gitShortstats;
        const { [id]: _p, ...restPrStates } = s.prStates;
        const { [id]: _c, ...restPrChecks } = s.prChecks;
        const { [id]: _pc, ...restPrComments } = s.prComments;
        const { [id]: _cs, ...restComposerSeeds } = s.composerSeeds;
        const { [id]: _d, ...restDelegations } = s.gitDelegations;
        return {
          workspace: fresh ?? s.workspace,
          selectedAgentId: s.selectedAgentId === id ? null : s.selectedAgentId,
          managedLogs: restLogs,
          transcriptLoading: restTranscriptLoading,
          transcriptLoaded: restTranscriptLoaded,
          managedBusy: restBusy,
          usage: restUsage,
          gitStates: restGitStates,
          gitShortstats: restShortstats,
          prStates: restPrStates,
          prChecks: restPrChecks,
          prComments: restPrComments,
          composerSeeds: restComposerSeeds,
          gitDelegations: restDelegations,
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
    set((s) => ({ transcriptLoading: { ...s.transcriptLoading, [id]: true } }));
    try {
      const provider = providerFor(get(), id);
      // session_records is the sole canonical store: per-provider verbatim
      // transcript bodies, rendered via normalizeTranscript→reduce. If a session
      // has no records yet (first open, or pre-cutover history), lazily ingest
      // its on-disk transcript and re-read. No-op for agents with no transcript.
      let records = await api.readSessionRecords(id);
      if (records.length === 0) {
        await api.syncSession(id);
        records = await api.readSessionRecords(id);
      }
      // Overlay outgoing-turn attachments + any undelivered (pending) turns, so
      // a failed send still shows on reload even when there are no records yet.
      const turns = await api.readUserTurns(id);
      const items = applyUserTurns(reduceRecords(provider, records), turns);
      const usage = usageFromRecords(provider, records);
      set((state) => {
        // Nothing stored but a live turn is already rendering — don't clobber it.
        if (items.length === 0 && (state.managedLogs[id]?.length ?? 0) > 0) {
          return {};
        }
        return {
          managedLogs: { ...state.managedLogs, [id]: items },
          managedBusy: { ...state.managedBusy, [id]: false },
          // Only overwrite when records carried usage — cursor folds usage
          // live, so an empty records result must not wipe it.
          ...(hasUsage(usage) ? { usage: { ...state.usage, [id]: usage } } : {}),
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

  seedComposer: (agentId, text) => {
    set((s) => {
      const pending = s.composerSeeds[agentId];
      const next = pending ? `${pending}\n\n${text}` : text;
      return { composerSeeds: { ...s.composerSeeds, [agentId]: next } };
    });
  },

  consumeComposerSeed: (agentId) => {
    set((s) => {
      if (!(agentId in s.composerSeeds)) return s;
      const { [agentId]: _dropped, ...rest } = s.composerSeeds;
      return { composerSeeds: rest };
    });
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

  clearError: () => set({ lastError: null }),

  setUpdateReady: (version) => set({ updateReadyVersion: version }),
  dismissUpdate: () => set({ updateReadyVersion: null }),

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
      drafts: [draft, ...s.drafts],
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

  spawnFromDraft: async (id, text, provider, attachments = [], thinking?) => {
    const draft = get().drafts.find((d) => d.id === id);
    if (!draft) return;
    set({ busy: true, lastError: null });
    const turnId = crypto.randomUUID();
    try {
      const view = get().viewMode;
      // `thinking` carries the composer's effort selection. For claude it's a
      // session-level spawn flag (--effort), applied here; per-turn agents
      // ignore it at spawn and take it per-turn via sendUserMessage below.
      const rec = await api.spawnAgent(
        view,
        draft.repoPath,
        provider,
        draft.name,
        thinking,
      );
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
            [rec.id]: [
              attachments.length > 0
                ? { kind: "user_message", text, attachments }
                : { kind: "user_message", text },
            ],
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
          api.sendUserMessage(rec.id, turnId, text, attachments, thinking),
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
  openOnboarding: () => set({ onboardingOpen: true }),
  closeOnboarding: () => {
    set({ onboardingOpen: false, onboardingComplete: true });
    setSetting("onboardingComplete", "true");
  },
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
  refreshAccount: async () => {
    try {
      const row = await getAccount();
      if (row) set({ account: toProfile(row) });
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
  refreshProviderVersions: async () => {
    try {
      const probes = await api.probeProviderVersions();
      const versions: Record<string, string> = {};
      const paths: Record<string, string> = {};
      for (const probe of probes) {
        if (probe.version) versions[probe.id] = probe.version;
        if (probe.path) paths[probe.id] = probe.path;
      }
      set({ providerVersions: versions, providerPaths: paths });
    } catch {
      // Non-fatal — UI falls back to hardcoded versions.
    }
  },
  refreshModelCatalog: async () => {
    // With no cached catalog yet (first run), seed from the packaged resource
    // on disk so lookups work before/without the network fetch.
    if (Object.keys(get().modelCatalog).length === 0) {
      const packaged = await loadPackagedCatalog();
      if (packaged) set({ modelCatalog: packaged });
    }
    const fresh = await refreshCatalog();
    if (fresh) set({ modelCatalog: fresh });
  },
  setViewMode: (v) => {
    set({ viewMode: v });
    setSetting("viewMode", v);
  },
}));
