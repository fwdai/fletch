import type { StateCreator } from "zustand";
import type { ChatItem } from "@/adapters";
import type { AgentUsage } from "@/adapters/usage";
import type {
  AgentRecord,
  AgentView,
  ContainerAuthStatus,
  DockerProbe,
  ForkCode,
  ForkContext,
  GhStatus,
  GitState,
  PrChecks,
  PrComments,
  PrState,
  RunPhase,
  ShortStats,
  Workspace,
} from "@/api";
import type { GitDelegation, GitDelegationKind } from "@/components/RightPanel/delegation";
import type { GitCommitAction } from "@/components/RightPanel/primaryActions";
import type { ModelMeta, SlimCatalog } from "@/data/modelCatalog";
import type { AccountProfile } from "@/storage/accounts";
import type { CustomAgent, NewCustomAgent } from "@/storage/customAgents";
import type { McpServer, NewMcpServer } from "@/storage/mcpServers";
import type {
  Density,
  FeatureFlags,
  SandboxEngine,
  SettingsIntent,
  SettingsSection,
  ThemeMode,
  WorkspaceView,
} from "@/storage/preferences";
import type { NewSkill, Skill } from "@/storage/skills";
import type { DraftAgent } from "./drafts";

/** A degraded transcript-ingest state stored per agent (the `healthy` status is
 *  never stored — it deletes the key). `provider`/`version` are for the banner
 *  copy; `status` picks the message. */
export interface SyncHealthInfo {
  status: "no_root" | "format_drift" | "read_error" | "partial_read";
  provider: string;
  version: string | null;
}

export interface AppSlice {
  busy: boolean;
  lastError: string | null;
  initialized: boolean;
  /** Version string of an update that's been downloaded + staged and is
   *  waiting for a restart to take effect. `null` = none pending. */
  updateReadyVersion: string | null;
  /** Release notes for the staged update (the manifest's `notes` field), shown
   *  in the restart toast. `null` when the manifest carried none. */
  updateReadyNotes: string | null;
  /** Transient status of a *manual* "Check for Updates…" run (menu-triggered),
   *  driving the feedback toast. `null` = idle. A found update transitions to
   *  `updateReadyVersion` instead. */
  updateCheckStatus: "checking" | "uptodate" | "error" | null;

  init: () => Promise<void>;
  clearError: () => void;
  /** Surface a message in the global error banner. For components (which can't
   *  call `set`) to report a failure they'd otherwise have to swallow. */
  setLastError: (message: string) => void;
  /** Record that an update has been staged (drives the restart toast). */
  setUpdateReady: (version: string, notes: string | null) => void;
  /** Dismiss the restart toast. The staged update still applies on next launch. */
  dismissUpdate: () => void;
  /** Run an on-demand update check (from the "Check for Updates…" menu),
   *  driving `updateCheckStatus` for feedback and staging any update found. */
  runUpdateCheck: () => Promise<void>;
}

export interface WorkspaceSlice {
  workspace: Workspace | null;
  selectedAgentId: string | null;
  /** A workflow run selected for the main pane, by run id. Mutually exclusive
   *  with selectedAgentId / activeDraftId. */
  selectedRunId: string | null;
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
  /** The backend's own start timestamp (epoch millis) for the current turn,
   *  from the `turn:started` event — the live-timer anchor. Shared with the
   *  persisted `started_at`, so the strip and footer measure from the identical
   *  instant; cleared at turn end. */
  turnStartedAt: Record<string, number>;
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
  /** Degraded transcript-ingest health per agent, keyed by agent_id, from the
   *  `session:sync-health` event. Absent = healthy (the common case): a `healthy`
   *  event deletes the key. Present = the vendor CLI drifted, so the chat view
   *  shows a non-blocking "couldn't read history" banner. In-memory only. */
  syncHealth: Record<string, SyncHealthInfo>;
  /** Per-agent cumulative token usage (and latest context-window fill),
   *  folded from session_records at turn-end and on transcript load. Keyed by
   *  agent_id; absent until the agent's first turn lands. Empty for agents that
   *  don't persist usage on disk (cursor, antigravity). See adapters/usage.ts. */
  usage: Record<string, AgentUsage>;
  /** Live run phase per agent, keyed by agent_id, from the `run:state` event
   *  stream. Absent = never started (read as "idle"). Fed by an app-wide
   *  subscription (not the RunPanel, which unmounts on tab switch) so the Run
   *  tab's "app is running" green dot stays lit from any tab. Single source of
   *  truth for phase — the RunPanel reads it rather than holding its own copy. */
  runPhases: Record<string, RunPhase>;
  /** Dev-server port per agent, keyed by agent_id. Written by the RunPanel when
   *  it resolves the run config (detected value + overrides), so the sidebar's
   *  running indicator can show `:port`. Absent until that agent's Run panel has
   *  been opened this session (the port isn't on the `run:state` event yet). */
  runPorts: Record<string, string>;

  selectAgent: (id: string | null) => void;
  /** Select a workflow run for the main pane (clears agent/draft/settings selection). */
  selectRun: (id: string) => void;
  spawn: (view: AgentView, repoPath: string) => Promise<AgentRecord | null>;
  /** Fork an existing workspace into a new one, seeding its worktree (`code`)
   *  and conversation (`context`) independently. Refreshes the workspace and
   *  selects the new agent. Resolves to the new record, or null on failure. */
  forkAgent: (
    parentId: string,
    code: ForkCode,
    context: ForkContext,
  ) => Promise<AgentRecord | null>;
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
  /** Record an agent's run phase (from a `run:state` event or a RunPanel
   *  snapshot rehydrate). Drives the Run tab's running indicator. */
  setRunPhase: (id: string, phase: RunPhase) => void;
  /** Record an agent's resolved dev-server port (from the RunPanel), for the
   *  sidebar's `:port` running indicator. */
  setRunPort: (id: string, port: string) => void;
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
  // Rename/relocate resolve on success and throw on failure, so the Project
  // Settings modal can show the error inline rather than in the global banner.
  /** Set a project's custom display name (independent of its folder). */
  renameProject: (projectId: string, name: string) => Promise<void>;
  /** Repoint a pinned repo at a moved folder, migrating its sidebar order and
   *  the open settings modal to the new path. */
  relocateProject: (oldPath: string, newPath: string) => Promise<void>;
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
    /** Also create + push to GitHub. False = local-only (no connection yet). */
    publish?: boolean,
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
  /** Refresh PR state for every agent with a known PR in one round-trip
   *  (used by the app-wide background poll). Backend emits `pr:state_changed`
   *  per agent, so the sidebar badge updates without opening the Git panel. */
  refreshAllPrStates: () => Promise<void>;
  /** Refresh CI checks for every agent with an open PR in one round-trip
   *  (used by the app-wide background poll) so the sidebar PR pill can tint
   *  pass/fail without opening the Git panel. */
  refreshAllPrChecks: () => Promise<void>;
  fetchPrChecks: (agentId: string) => Promise<void>;
  fetchPrComments: (agentId: string) => Promise<void>;
  delegateGitAction: (agentId: string, kind: GitDelegationKind, prompt: string) => void;
  markGitDelegationRunning: (agentId: string) => void;
  /** The agent ran a successful mutating git op `op` (backend
   *  `agent:git-action`). Sets the causal proof only if `op` matches the
   *  pending delegation's kind. */
  markGitDelegationActed: (agentId: string, op: string) => void;
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
  /** Publish a local-only project (no origin) to GitHub, then refresh git
   *  state so the panel switches out of the no-origin affordances. Resolves
   *  the repo web URL on success, null on error. */
  publishAgent: (agentId: string, isPrivate: boolean) => Promise<string | null>;
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

/** Right-rail panel tabs. Mirrors the `Tab` ids in RightPanel; kept here so the
 *  store can remember the last-open tab per agent without importing a component. */
export type RightPanelTab = "code" | "git" | "run" | "term";

export interface UiSlice {
  /** Quick-settings popover (gear / ⌘,). */
  settingsOpen: boolean;
  /** Dedicated full-screen settings surface (General / Account / Providers).
   *  Replaces the workspace panes while open. */
  settingsScreenOpen: boolean;
  settingsSection: SettingsSection;
  /** One-shot deep-link intent for the settings screen, consumed and cleared
   *  by the target pane on mount (e.g. open the new-custom-agent editor
   *  straight from the composer's agent picker). */
  settingsIntent: SettingsIntent | null;
  /** GitHub connect modal: a small app-level overlay that runs the OAuth
   *  device flow inline, so any "Connect GitHub" affordance (e.g. the Git
   *  panel) can start signing in on the first click instead of detouring
   *  through Settings. */
  githubConnectOpen: boolean;
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
  /** Project Settings modal: a centered overlay (History-style) for editing
   *  per-project defaults. Keyed by the sidebar's repo path — the modal
   *  resolves the project_id on open. Open iff non-null. */
  projectSettingsRepoPath: string | null;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  leftWidth: number;
  rightWidth: number;
  /** Last-open right-rail tab per agent, keyed by agent id. Lets the panel
   *  restore the tab the user was on (e.g. Git) when they switch back to an
   *  agent, instead of always resetting to the first tab. In-memory only. */
  rightPanelTabs: Record<string, RightPanelTab>;

  toggleSettings: (open?: boolean) => void;
  openSettingsScreen: (section?: SettingsSection, intent?: SettingsIntent) => void;
  closeSettingsScreen: () => void;
  setSettingsSection: (section: SettingsSection) => void;
  /** Clear a consumed `settingsIntent` so it fires only once. */
  clearSettingsIntent: () => void;
  /** Open / close the GitHub connect modal (the device flow starts on open). */
  openGithubConnect: () => void;
  closeGithubConnect: () => void;
  /** Open the onboarding overlay (e.g. "Replay tour" from Settings). */
  openOnboarding: () => void;
  /** Dismiss onboarding and mark it complete so it won't auto-open again. */
  closeOnboarding: () => void;
  toggleHistory: (open?: boolean) => void;
  selectHistoryAgent: (id: string | null) => void;
  /** Open the Project Settings modal for a sidebar repo group. */
  openProjectSettings: (repoPath: string) => void;
  closeProjectSettings: () => void;
  toggleLeft: () => void;
  toggleRight: () => void;
  /** Live (in-memory) width update during a splitter drag. */
  setLeftWidth: (w: number) => void;
  setRightWidth: (w: number) => void;
  /** Persist the final width once a splitter drag ends. */
  commitLeftWidth: (w: number) => void;
  commitRightWidth: (w: number) => void;
  /** Remember the right-rail tab an agent was last viewing. */
  setRightPanelTab: (agentId: string, tab: RightPanelTab) => void;
}

export interface AccountSlice {
  /** Local account profile, loaded on init. `null` until the row is read. */
  account: AccountProfile | null;
  /** Anonymous usage telemetry consent. Opt-out: defaults on. */
  telemetryEnabled: boolean;
  /** GitHub connection: null until the first probe, then the live status.
   *  `authenticated` gates push/PR/clone affordances app-wide. */
  github: GhStatus | null;

  saveAccount: (patch: Pick<AccountProfile, "firstName" | "lastName" | "email">) => Promise<void>;
  /** Re-read the local account row into the store — e.g. after an OAuth
   *  sign-in writes the provider profile to SQLite. */
  refreshAccount: () => Promise<void>;
  /** Re-probe the GitHub connection into `github` (after sign-in/disconnect,
   *  and once on init). */
  refreshGithub: () => Promise<void>;
  /** Drop the stored GitHub token and return to local-only mode. */
  disconnectGithub: () => Promise<void>;
  setTelemetryEnabled: (enabled: boolean) => void;
}

/** Live state of the embedded docker image build (first docker spawn). `null`
 *  when no build is in progress. `building` streams the latest output line;
 *  `failed` stays up (with `error`) until dismissed. Success clears to `null`. */
export interface DockerBuildProgress {
  status: "building" | "failed";
  /** Most recent `docker build` output line (building only). */
  lastLine: string | null;
  /** Failure reason (failed only). */
  error: string | null;
}

export interface SandboxSlice {
  /** Engine new agents are stamped with. Mirrors the backend-owned
   *  `sandbox_engine` setting; existing agents keep their stamped engine. */
  sandboxEngine: SandboxEngine;
  /** Latest Docker availability probe; `null` until the first probe lands. */
  dockerProbe: DockerProbe | null;
  /** Which container auth chain step is active (Anthropic credentials for
   *  docker agents); `null` until the first refresh lands. */
  containerAuth: ContainerAuthStatus | null;
  /** Live image-build progress for the build toast; `null` = no build. */
  dockerBuild: DockerBuildProgress | null;
  /** Advanced docker launch knobs, mirrored from the backend-owned
   *  `docker_image` / `docker_memory` / `docker_cpus` settings. Empty string =
   *  unset (the launch path uses its defaults: 4g memory, 2 cpus, embedded
   *  image). */
  dockerImage: string;
  dockerMemory: string;
  dockerCpus: string;

  /** Persist a new engine choice via the backend, which validates docker
   *  against a live daemon probe — reverts the store on refusal. */
  setSandboxEngine: (engine: SandboxEngine) => Promise<void>;
  /** Re-probe Docker availability into `dockerProbe` (settings pane open).
   *  Returns the probe result so callers can poll until the daemon answers. */
  refreshDockerProbe: () => Promise<DockerProbe>;
  /** Re-resolve the container auth chain into `containerAuth`. */
  refreshContainerAuth: () => Promise<void>;
  /** Persist a pasted `claude setup-token` for containers, then refresh the
   *  status. Throws on backend refusal (empty token) so the connect modal
   *  can show the error inline. */
  setContainerAuthToken: (token: string) => Promise<void>;
  /** Drop the stored container token, then refresh the status (a later chain
   *  step — shell env / credentials file — may take over). */
  clearContainerAuthToken: () => Promise<void>;
  /** Dismiss the build toast (used after a failed build). */
  dismissDockerBuild: () => void;
  /** Persist the advanced docker launch knobs (image / memory / cpus) via the
   *  backend, which writes the settings AND updates the spawn-path mirror.
   *  Reverts the store on failure. */
  saveDockerLaunchSettings: (image: string, memory: string, cpus: string) => Promise<void>;
  /** Launch Docker Desktop (daemon-down error action); surfaces failures via
   *  `lastError`. */
  startDockerDesktop: () => Promise<void>;
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
  /** Play the chime when an agent turn finishes or needs input while you're
   *  not watching that chat. Opt-out. */
  soundEnabled: boolean;
  /** Send a native OS notification when an agent turn finishes or needs input
   *  while you're not watching that chat. Opt-out. */
  notifyEnabled: boolean;
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
  setSoundEnabled: (on: boolean) => void;
  setNotifyEnabled: (on: boolean) => void;
  setViewMode: (v: WorkspaceView) => void;
}

export interface ProvidersSlice {
  providerFlags: Record<string, boolean>;
  /** Live-probed version strings keyed by provider id. Populated async on
   *  init; absent until a probe resolves (never a hardcoded default). */
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
  /** Sticky custom-agent selection for the next new draft (persisted). */
  newDraftCustomAgentId?: string;
  /** The project a new agent was last started in (persisted). Seeds ⌘N's
   *  default project; validated against the live repo list on use. */
  lastRepoPath?: string;

  // drafts
  createDraft: (repoPath: string) => Promise<void>;
  /** Remember the last project an agent was started in and persist it. */
  setLastRepoPath: (repoPath: string) => void;
  updateDraft: (id: string, patch: Partial<DraftAgent>) => void;
  removeDraft: (id: string) => void;
  selectDraft: (id: string | null) => void;
  setNewDraftSelection: (provider: string, model?: string, customAgentId?: string) => void;
  rerollDraftName: (id: string) => Promise<void>;
  /** Spawn the real agent for a draft and dispatch the first message. */
  spawnFromDraft: (
    id: string,
    text: string,
    provider: string,
    model?: string,
    attachments?: string[],
    thinking?: string,
    customAgentId?: string,
  ) => Promise<void>;
}

export interface CustomAgentsSlice {
  /** User-defined agent presets, mirrored from the `custom_agents` table and
   *  ordered newest-edited first. Loaded once on init. */
  customAgents: CustomAgent[];

  loadCustomAgents: () => Promise<void>;
  createCustomAgent: (agent: NewCustomAgent) => Promise<CustomAgent>;
  /** Patch an existing custom agent; resolves to the merged row, or null if the
   *  id is unknown. */
  updateCustomAgent: (id: string, patch: Partial<NewCustomAgent>) => Promise<CustomAgent | null>;
  deleteCustomAgent: (id: string) => Promise<void>;
  /** Clone a custom agent ("… copy"); resolves to the new row, or null if the
   *  source id is unknown. */
  duplicateCustomAgent: (id: string) => Promise<CustomAgent | null>;
}

export interface SkillsSlice {
  /** Shared skills library, mirrored from the `skills` table and ordered
   *  newest-edited first. Loaded once on init. */
  skills: Skill[];

  loadSkills: () => Promise<void>;
  createSkill: (skill: NewSkill) => Promise<Skill>;
  /** Patch an existing skill; resolves to the merged row, or null if the id is
   *  unknown. */
  updateSkill: (id: string, patch: Partial<NewSkill>) => Promise<Skill | null>;
  /** Delete a skill and detach its id from every custom agent. */
  deleteSkill: (id: string) => Promise<void>;
}

export interface McpServersSlice {
  /** Shared MCP server registry, mirrored from the `mcp_servers` table and
   *  ordered newest-edited first. Loaded once on init. */
  mcpServers: McpServer[];

  loadMcpServers: () => Promise<void>;
  createMcpServer: (server: NewMcpServer) => Promise<McpServer>;
  /** Patch an existing server; resolves to the merged row, or null if the id is
   *  unknown. */
  updateMcpServer: (id: string, patch: Partial<NewMcpServer>) => Promise<McpServer | null>;
  /** Delete a server and detach its id from every custom agent. */
  deleteMcpServer: (id: string) => Promise<void>;
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
  ProvidersSlice &
  CustomAgentsSlice &
  SkillsSlice &
  McpServersSlice &
  SandboxSlice;

export type SliceCreator<T> = StateCreator<AppState, [], [], T>;
