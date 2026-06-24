import type { StateCreator } from "zustand";
import type {
  AgentRecord,
  AgentView,
  GitState,
  PrChecks,
  PrComments,
  PrState,
  ShortStats,
  Workspace,
} from "../api";
import type {
  GitDelegation,
  GitDelegationKind,
} from "../components/RightPanel/delegation";
import type { GitCommitAction } from "../components/RightPanel/primaryActions";
import type { ChatItem } from "../adapters";
import type { AgentUsage } from "../adapters/usage";
import type { SlimCatalog, ModelMeta } from "../data/modelCatalog";
import type {
  ThemeMode,
  Density,
  WorkspaceView,
  SettingsSection,
  FeatureFlags,
} from "../storage/preferences";
import type { AccountProfile } from "../storage/accounts";
import type { DraftAgent } from "./drafts";

export interface AppSlice {
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  /** Version string of an update that's been downloaded + staged and is
   *  waiting for a restart to take effect. `null` = none pending. */
  updateReadyVersion: string | null;

  init: () => Promise<void>;
  clearError: () => void;
  /** Surface a message in the global error banner. For components (which can't
   *  call `set`) to report a failure they'd otherwise have to swallow. */
  setLastError: (message: string) => void;
  /** Record that an update has been staged (drives the restart toast). */
  setUpdateReady: (version: string) => void;
  /** Dismiss the restart toast. The staged update still applies on next launch. */
  dismissUpdate: () => void;
}

export interface WorkspaceSlice {
  workspace: Workspace | null;
  selectedAgentId: string | null;
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

  selectAgent: (id: string | null) => void;
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
}

export interface ReposSlice {
  addWorkspaceRepo: (path: string) => Promise<void>;
  removeWorkspaceRepo: (path: string) => Promise<void>;
  /** Open the log folder in the OS file manager; surfaces failures via
   *  `lastError` rather than swallowing them. */
  revealLogs: () => Promise<void>;
  // Clone/create resolve on success and throw on failure, so the New Project
  // modal can show the error inline rather than in the global banner.
  cloneRepo: (spec: string, destParent: string) => Promise<void>;
  createRepo: (
    name: string,
    destParent: string,
    isPrivate: boolean,
    description?: string,
  ) => Promise<void>;
}

export interface GitSlice {
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
  /** PR state per agent, keyed by agent_id. Updated by the pr:state_changed watcher event. */
  prStates: Record<string, PrState | null>;
  /** Rich PR merge-gate + checks per agent. Absent key = not yet fetched;
   *  `null` = confirmed unavailable (no PR / gh failure). */
  prChecks: Record<string, PrChecks | null>;
  /** Unresolved PR review comments per agent. Absent = not yet fetched;
   *  `null` = confirmed unavailable (no PR / gh failure). */
  prComments: Record<string, PrComments | null>;
  /** Active agent-delegated git action per agent (absent = none). Set when a
   *  panel action hands control to the agent; cleared by the panel when the
   *  watched git/PR transition lands or the agent gives up. */
  gitDelegations: Record<string, GitDelegation>;
  /** Sticky changes-state commit mode (Commit / & push / & open PR). Global
   *  across workspaces, persisted in settings until the user picks another. */
  gitCommitAction: GitCommitAction;

  /** Fetch full git state for one agent (used by the focused panel's poll). */
  fetchGitState: (agentId: string) => Promise<void>;
  /** Fetch compact shortstats for every live agent in one round-trip
   *  (used by the app-wide background poll). */
  fetchAllShortstats: () => Promise<void>;
  fetchPrState: (agentId: string) => Promise<void>;
  fetchPrChecks: (agentId: string) => Promise<void>;
  fetchPrComments: (agentId: string) => Promise<void>;
  delegateGitAction: (agentId: string, kind: GitDelegationKind, prompt: string) => void;
  markGitDelegationRunning: (agentId: string) => void;
  /** The pre-existing turn the delegation was queued behind has settled —
   *  drop `queued` and restart the give-up clock for our own turn. */
  markGitDelegationDequeued: (agentId: string) => void;
  clearGitDelegation: (agentId: string) => void;
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
}

export interface ComposerSlice {
  /** Pending text to push into an agent's chat composer (the "→ chat" quick
   *  action on a review comment). Generic, single-channel: a new seed for an
   *  agent appends to any unconsumed one. The Composer applies and clears it. */
  composerSeeds: Record<string, string>;
  /** Unsent composer text, keyed by agent id (existing chats) or draft id
   *  (the new-agent composer). Switching views remounts the Composer and would
   *  otherwise drop what the user typed; this preserves it until sent. Set to
   *  "" to clear an entry. */
  composerDrafts: Record<string, string>;

  seedComposer: (agentId: string, text: string) => void;
  consumeComposerSeed: (agentId: string) => void;
  setComposerDraft: (key: string, text: string) => void;
}

export interface UiSlice {
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

  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Open the onboarding overlay (e.g. "Replay tour" from Settings). */
  openOnboarding: () => void;
  /** Dismiss onboarding and mark it complete so it won't auto-open again. */
  closeOnboarding: () => void;
  toggleHistory: (open?: boolean) => void;
  selectHistoryAgent: (id: string | null) => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  /** Live (in-memory) width update during a splitter drag. */
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;
  /** Persist the final width once a splitter drag ends. */
  commitLeftWidth: (w: number) => void;
  commitRightWidth: (w: number) => void;
}

export interface AccountSlice {
  /** Local account profile, loaded on init. `null` until the row is read. */
  account: AccountProfile | null;
  /** Anonymous usage telemetry consent. Opt-out: defaults on. */
  telemetryEnabled: boolean;

  saveAccount: (
    patch: Pick<AccountProfile, "firstName" | "lastName" | "email">,
  ) => Promise<void>;
  /** Re-read the local account row into the store — e.g. after an OAuth
   *  sign-in writes the provider profile to SQLite. */
  refreshAccount: () => Promise<void>;
  setTelemetryEnabled: (enabled: boolean) => void;
}

export interface AppearanceSlice {
  // ── appearance & feature flags ────────────────────────────────────────────
  theme: ThemeMode;
  /** Syntax-highlighting theme for the File panel editor. "quorum" = the
   *  built-in palette; other ids map to a highlight.js theme family that
   *  follows the app's light/dark mode. See data/codeThemes.ts. */
  codeTheme: string;
  accent: string;
  density: Density;
  features: FeatureFlags;
  /** View mode preference for the workspace pane. Persisted; falls
   *  back to the agent's own `view` field for native vs. custom
   *  switching. */
  viewMode: WorkspaceView;

  // appearance
  setTheme: (t: ThemeMode) => void;
  setCodeTheme: (id: string) => void;
  setAccent: (a: string) => void;
  setDensity: (d: Density) => void;
  setFeature: <K extends keyof FeatureFlags>(k: K, v: FeatureFlags[K]) => void;
  setViewMode: (v: WorkspaceView) => void;
}

export interface ProvidersSlice {
  providerFlags: Record<string, boolean>;
  /** Live-probed version strings keyed by provider id. Populated async on
   *  init; falls back to hardcoded defaults in PROVIDERS when missing. */
  providerVersions: Record<string, string>;
  /** Resolved binary paths keyed by provider id, from the version probe. */
  providerPaths: Record<string, string>;
  /** True once a provider probe has *succeeded*. Stays false while probing and
   *  after a failed probe, so install-aware UI (model picker, readiness check)
   *  fails open — treating install state as unknown rather than "all missing" —
   *  instead of disabling every agent on a transient IPC error or on boot. */
  providersProbed: boolean;
  /** User-set custom binary paths keyed by provider id (the raw value entered,
   *  before resolution). Absent = auto-detect. This is the source of truth for
   *  the "Custom" tag in the providers settings, independent of the probe. */
  providerPathOverrides: Record<string, string>;
  /** Per-model metadata (context window, reasoning) keyed by bare model id —
   *  the `byId` view of the hybrid catalog. Seeded from the localStorage cache
   *  on init, rebuilt from agent discovery + models.dev when stale (24h). */
  modelCatalog: SlimCatalog;
  /** Supported models grouped by agent — the `byAgent` view, for the model
   *  picker. Same provenance and refresh cadence as `modelCatalog`. */
  modelsByAgent: Record<string, ModelMeta[]>;

  setProviderEnabled: (id: string, enabled: boolean) => void;
  /** Re-probe installed provider CLIs for versions + binary paths. Runs once
   *  on init and again when the user re-scans from the Providers settings. */
  refreshProviderVersions: () => Promise<void>;
  /** Set (path) or clear (null) a provider's custom binary path. Persists the
   *  override, updates local state, and re-probes so the version/path refresh. */
  setProviderPathOverride: (id: string, path: string | null) => Promise<void>;
  /** Rebuild the model catalog (agent discovery + models.dev) when the cache is
   *  stale (24h). Runs once on init; non-fatal on failure (keeps cached data). */
  refreshModelCatalog: () => Promise<void>;
}

export interface DraftsSlice {
  drafts: DraftAgent[];
  activeDraftId: string | null;
  newDraftProvider: string;
  newDraftModel?: string;

  // drafts
  createDraft: (repoPath: string) => Promise<void>;
  updateDraft: (id: string, patch: Partial<DraftAgent>) => void;
  removeDraft: (id: string) => void;
  selectDraft: (id: string | null) => void;
  setNewDraftSelection: (provider: string, model?: string) => void;
  rerollDraftName: (id: string) => Promise<void>;
  /** Spawn the real agent for a draft and dispatch the first message. */
  spawnFromDraft: (
    id: string,
    text: string,
    provider: string,
    model?: string,
    attachments?: string[],
    thinking?: string,
  ) => Promise<void>;
}

export type AppState = AppSlice &
  WorkspaceSlice &
  ReposSlice &
  GitSlice &
  ComposerSlice &
  DraftsSlice &
  UiSlice &
  AccountSlice &
  AppearanceSlice &
  ProvidersSlice;

export type SliceCreator<T> = StateCreator<AppState, [], [], T>;
