import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentModels } from "./data/modelCatalog/types";

export type AgentStatus = "spawning" | "running" | "idle" | "stopped" | "error";

export type AgentView = "custom" | "native";

export interface TrackedRepo {
  repo_path: string;
  subdir: string;
  branch?: string | null;
  parent_branch?: string | null;
}

export interface DiffStats {
  additions: number;
  deletions: number;
}

export interface ArchivedRepoSnapshot {
  repo_path: string;
  subdir: string;
  branch_name?: string | null;
  branch_tip_sha?: string | null;
  parent_branch?: string | null;
  parent_branch_sha?: string | null;
  diff_stats: DiffStats;
}

export interface ArchiveMetadata {
  archived_at: string;
  repos: ArchivedRepoSnapshot[];
  diff_stats: DiffStats;
}

export interface AgentRecord {
  id: string;
  /** Project this agent belongs to. Used to scope project-level
   *  settings (e.g. Run panel overrides). */
  project_id: string;
  name: string;
  /** Which CLI backend powers this agent (claude, codex, ...). Maps to
   *  the TS adapter registered under the same id. */
  provider: string;
  /** Repos this agent has worktrees in. Always non-empty;
   *  `repos[0]` is the primary (the workspace repo at spawn). */
  repos: TrackedRepo[];
  task: string;
  status: AgentStatus;
  view: AgentView;
  session_id?: string | null;
  created_at: string;
  last_error?: string | null;
  /** Set when the agent has been archived. Live agents have null. */
  archive?: ArchiveMetadata | null;
  /** Claude's session-level reasoning effort (`--effort <level>`), chosen at
   *  spawn. Null for agents where effort wasn't set or isn't session-scoped. */
  effort?: string | null;
  /** Model chosen at spawn. Null/undefined means the provider CLI default. */
  model?: string | null;
  /** The custom agent this session was spawned from (null for a built-in
   *  spawn). Used to show the custom agent's name/color in the sidebar. */
  custom_agent_id?: string | null;
}

export interface Workspace {
  /** Repos pinned in the sidebar. Empty on first launch. */
  repos: string[];
  agents: AgentRecord[];
}

export interface AgentOutputEvent {
  agent_id: string;
  /** Raw PTY bytes, base64-encoded over IPC. Decode with `decodeBase64`. */
  bytes: string;
}

/** Raw stream-json event from claude in custom view. Shape varies by
 *  `type`; the UI pattern-matches. */
export interface AgentManagedEvent {
  agent_id: string;
  event: Record<string, unknown> & { type?: string };
}

export interface AgentStatusEvent {
  agent_id: string;
  status: AgentStatus;
  last_error: string | null;
}

export interface AgentViewEvent {
  agent_id: string;
  view: AgentView;
}

export interface AgentTaskEvent {
  agent_id: string;
  task: string;
}

export interface AgentBranchEvent {
  agent_id: string;
  subdir: string;
  branch: string;
}

export interface AgentRepoAddedEvent {
  agent_id: string;
  repo: TrackedRepo;
}

/** A successful mutating git op (`commit`/`push`/`PR`/`update`) the agent ran
 *  this turn — the ground-truth signal that a delegated git action happened. */
export interface AgentGitActionEvent {
  agent_id: string;
  op: string;
}

export type StatusKind = "modified" | "added" | "deleted" | "renamed" | "untracked" | "conflicted";

export interface FileStatus {
  path: string;
  kind: StatusKind;
  staged: boolean;
  additions: number;
  deletions: number;
}

/** Compact projection of GitState used by the app-wide bulk poll —
 *  enough for sidebar shortstats and tab badges without shipping every
 *  agent's file list over IPC. */
export interface ShortStats {
  additions: number;
  deletions: number;
  file_count: number;
}

export interface GitState {
  branch: string;
  parent_branch: string;
  ahead: number;
  behind: number;
  /** Commits on HEAD not yet on the upstream — how many a push would send.
   *  Distinct from `ahead` (measured vs the base branch). */
  unpushed: number;
  files: FileStatus[];
  additions: number;
  deletions: number;
  /** GitHub web base for `origin` (`https://github.com/owner/repo`), or null
   *  when origin is missing / not a github.com remote. Lets the panel link to
   *  a commit or compare view. */
  remote_url?: string | null;
  /** Whether an `origin` remote exists at all (GitHub or not). False = a
   *  local-only repo: push/PR give way to "Publish to GitHub". */
  has_origin: boolean;
  /** HEAD commit SHA, for a single-commit link when one commit is ahead. */
  head_sha?: string | null;
}

/** One file in the worktree, as returned by `list_worktree_tree`.
 *  `status` is the single-letter git status vs the parent branch
 *  ("M" | "A" | "D" | "R"), or null when the file is unchanged. */
export interface WorktreeFile {
  path: string;
  status: string | null;
  additions: number;
  deletions: number;
}

/** One entry in an arbitrary directory listing, used by the composer's `@`
 *  file-mention autocomplete when the user types a filesystem path. */
export interface DirEntry {
  name: string;
  is_dir: boolean;
}

/** A directory listing plus the absolute (tilde-expanded) path that was
 *  read, returned by `list_dir`. */
export interface DirListing {
  base: string;
  entries: DirEntry[];
}

/** A worktree file's contents plus the metadata the File-panel editor
 *  needs. `chg_add` / `chg_mod` are 1-indexed line numbers the agent
 *  added / modified (drives the change gutter). */
export interface WorktreeFileContents {
  text: string;
  lang: string;
  status: string | null;
  chg_add: number[];
  chg_mod: number[];
  binary: boolean;
  too_large: boolean;
}

export type PrStatus = "open" | "merged" | "closed";

export interface PrState {
  number: number;
  url: string;
  state: PrStatus;
  title: string;
  mergeable: boolean;
}

export interface PrStateChangedEvent {
  agent_id: string;
  state: PrState | null;
}

/** Lightweight PR summary for the composer's "#" mention autocomplete. */
export interface PrSummary {
  number: number;
  title: string;
  state: PrStatus;
}

/** GitHub's combined merge gate (`mergeStateStatus`), normalized (spec §6). */
export type MergeState =
  | "clean"
  | "blocked"
  | "unstable"
  | "behind"
  | "dirty"
  | "draft"
  | "has_hooks"
  | "unknown";

/** One CI check, normalized from gh's statusCheckRollup. */
export interface CheckRun {
  name: string;
  status: "queued" | "in_progress" | "completed";
  conclusion: string | null;
  required: boolean;
  url: string | null;
  started_at: string | null;
  completed_at: string | null;
}

/** Rich PR merge-gate + per-check detail. Heavier than PrState — polled on
 *  a slow cadence while a PR is open. */
export interface PrChecks {
  merge_state: MergeState;
  rollup: "none" | "pending" | "passing" | "failing";
  total: number;
  passed: number;
  failed: number;
  pending: number;
  required_failing: string[];
  runs: CheckRun[];
}

/** One unresolved PR review thread, flattened to its root comment. */
export interface PrComment {
  author: string;
  /** Author is a GitHub App / bot (Greptile, CodeRabbit, …). Bots phrase
   *  their comments for an AI already, so the panel inserts them as-is;
   *  human comments get a file/line context wrapper. */
  is_bot: boolean;
  body: string;
  path: string | null;
  line: number | null;
  url: string;
  /** Replies after the root comment. */
  replies: number;
}

/** Unresolved review threads for a PR — polled on the slow checks cadence. */
export interface PrComments {
  unresolved: PrComment[];
}

export type RunPhase = "idle" | "setup" | "running" | "stopped";

export interface RunStateSnapshot {
  phase: RunPhase;
  last_error: string | null;
  /** Raw PTY bytes accumulated since the panel was last cleared.
   *  Sent as a JSON array of u8 values; decode with TextDecoder. */
  log: number[];
}

/** A single detected run-config row (see Rust `run_detect::DetectedRow`). */
export interface DetectedRow {
  /** "version" | "install" | "dev" | "test" | "build" | "port" | "env" */
  id: string;
  group: "environment" | "scripts" | "server";
  key: string;
  value: string;
  source: string;
}

/** One ecosystem's detected config (see Rust `run_detect::DetectedConfig`). */
export interface DetectedConfig {
  ecosystem: string;
  confidence: number;
  rows: DetectedRow[];
}

export interface RunOutputEvent {
  agent_id: string;
  bytes: number[];
}

export interface RunStateEvent {
  agent_id: string;
  phase: RunPhase;
  last_error: string | null;
}

export interface ProviderProbe {
  id: string;
  version: string | null;
  path: string | null;
}

/** Presence of a required non-agent CLI (e.g. `git`) for the readiness check. */
export interface ToolStatus {
  installed: boolean;
  version: string | null;
  path: string | null;
  /** Which git resolution chose: the user's own install or the portable dist
   *  the app downloaded. Null for plain PATH-resolved tools. */
  source: "system" | "portable" | null;
}

/** Result of pre-flighting a custom agent binary path before saving it as an
 *  override. `executable` is whether the path is a runnable file; `version` is
 *  what `<path> --version` reported (null if it didn't run or didn't parse). */
export interface BinValidation {
  executable: boolean;
  version: string | null;
}

/** Whether the `gh` CLI is installed and authenticated (New Project flow). */
export interface GhStatus {
  installed: boolean;
  authenticated: boolean;
  login: string | null;
}

/** One repo from `gh repo list`, for the clone picker. */
export interface GhRepoSummary {
  name_with_owner: string;
  description: string | null;
  is_private: boolean;
  updated_at: string;
}

/** An editor or terminal detected on the user's machine (title-bar launcher). */
export interface DetectedEditor {
  id: string;
  label: string;
  kind: "editor" | "terminal";
}

export const api = {
  getWorkspace: () => invoke<Workspace | null>("get_workspace"),
  revealLogs: () => invoke<void>("reveal_logs"),
  /** Editors installed on this machine, in picker order. */
  detectEditors: () => invoke<DetectedEditor[]>("detect_editors"),
  /** Open an agent's worktree in the chosen editor. */
  openInEditor: (agentId: string, editorId: string) =>
    invoke<void>("open_in_editor", { agentId, editorId }),
  // Anonymous usage telemetry. Persists the opt-out flag and toggles the live
  // pipeline (events themselves are emitted from the backend).
  setTelemetryEnabled: (enabled: boolean) => invoke<void>("set_telemetry_enabled", { enabled }),
  // Emit the deferred first `app_opened` once onboarding completes — i.e. after
  // the data-sharing disclosure has been shown. See `track_app_opened` (Rust).
  trackAppOpened: () => invoke<void>("track_app_opened"),
  getAgentDiffStats: (agentId: string) => invoke<DiffStats>("get_agent_diff_stats", { agentId }),
  addWorkspaceRepo: (repoPath: string) => invoke<Workspace>("add_workspace_repo", { repoPath }),
  removeWorkspaceRepo: (repoPath: string) =>
    invoke<Workspace>("remove_workspace_repo", { repoPath }),
  ghStatus: () => invoke<GhStatus>("gh_status"),
  ghRepoList: () => invoke<GhRepoSummary[]>("gh_repo_list"),
  cloneRepo: (spec: string, destParent: string) =>
    invoke<Workspace>("clone_repo", { spec, destParent }),
  createRepo: (
    name: string,
    destParent: string,
    isPrivate: boolean,
    description?: string,
    publish?: boolean,
  ) =>
    invoke<Workspace>("create_repo", {
      name,
      destParent,
      private: isPrivate,
      description: description ?? null,
      publish: publish ?? true,
    }),
  publishAgent: (agentId: string, isPrivate: boolean) =>
    invoke<string>("publish_agent", { agentId, private: isPrivate }),
  githubDisconnect: () => invoke<void>("github_disconnect"),
  spawnAgent: (
    view: AgentView,
    repoPath: string,
    provider?: string,
    name?: string,
    effort?: string,
    model?: string,
    instructions?: string,
    customAgentId?: string,
    /** Base the worktree forks from and the agent's recorded parent branch
     *  (PR base / ahead-behind). The new-agent screen passes the chosen base
     *  branch; a workflow step instead passes the previous step's HEAD
     *  (commit-ish) so its worktree continues that work. */
    forkBase?: string,
  ) =>
    invoke<AgentRecord>("spawn_agent", {
      view,
      repoPath,
      provider,
      name,
      effort: effort ?? null,
      model: model ?? null,
      instructions: instructions ?? null,
      customAgentId: customAgentId ?? null,
      forkBase: forkBase ?? null,
    }),
  writeToAgent: (agentId: string, data: string) =>
    invoke<void>("write_to_agent", { agentId, data }),
  /** Resolves to `true` when the message was enqueued for a later turn boundary
   *  rather than delivered now (injected live / sent as a new turn). */
  sendUserMessage: (
    agentId: string,
    turnId: string,
    text: string,
    attachments: string[] = [],
    thinking?: string,
  ) =>
    invoke<boolean>("send_user_message", {
      agentId,
      turnId,
      text,
      attachments,
      thinking: thinking ?? null,
    }),
  answerToolUse: (
    agentId: string,
    requestId: string,
    updatedInput: unknown,
    behavior: "allow" | "deny" = "allow",
    message?: string,
  ) =>
    invoke<void>("answer_tool_use", {
      agentId,
      requestId,
      updatedInput,
      behavior,
      message: message ?? null,
    }),
  resizeAgent: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_agent", { agentId, cols, rows }),
  switchView: (agentId: string, view: AgentView) => invoke<void>("switch_view", { agentId, view }),
  resumeAgent: (agentId: string) => invoke<void>("resume_agent", { agentId }),
  stopAgent: (agentId: string) => invoke<void>("stop_agent", { agentId }),
  discardAgent: (agentId: string) => invoke<void>("discard_agent", { agentId }),
  archiveAgent: (agentId: string) => invoke<void>("archive_agent", { agentId }),
  restoreAgent: (agentId: string) => invoke<void>("restore_agent", { agentId }),
  readSessionRecords: (agentId: string) =>
    invoke<SessionRecord[]>("read_session_records", { agentId }),
  readUserTurns: (agentId: string) => invoke<UserTurn[]>("read_user_turns", { agentId }),
  syncSession: (agentId: string) => invoke<void>("sync_session", { agentId }),
  /** Persist a runtime-compiled record (e.g. cursor's per-turn usage from its
   *  live `result` event) into session_records. Idempotent on `nativeId`. */
  appendLiveRecord: (
    agentId: string,
    provider: string,
    nativeId: string,
    body: Record<string, unknown>,
  ) => invoke<boolean>("append_live_record", { agentId, provider, nativeId, body }),
  addRepoToAgent: (agentId: string, repoPath: string) =>
    invoke<TrackedRepo>("add_repo_to_agent", { agentId, repoPath }),
  allocateDraftName: (used: string[]) => invoke<string>("allocate_draft_name", { used }),
  getGitState: (agentId: string) => invoke<GitState | null>("get_git_state", { agentId }),
  getAllShortstats: () => invoke<Record<string, ShortStats>>("get_all_shortstats"),
  getPrState: (agentId: string) => invoke<PrState | null>("get_pr_state", { agentId }),
  refreshAllPrStates: () => invoke<Record<string, PrState | null>>("refresh_all_pr_states"),
  refreshAllPrChecks: () => invoke<Record<string, PrChecks | null>>("refresh_all_pr_checks"),
  getPrChecks: (agentId: string) => invoke<PrChecks | null>("get_pr_checks", { agentId }),
  getPrComments: (agentId: string) => invoke<PrComments | null>("get_pr_comments", { agentId }),
  pushAgent: (agentId: string) => invoke<string>("push_agent", { agentId }),
  pullAgent: (agentId: string) => invoke<void>("pull_agent", { agentId }),
  rebaseAgent: (agentId: string) => invoke<void>("rebase_agent", { agentId }),
  commitAgent: (agentId: string, message: string) =>
    invoke<void>("commit_agent", { agentId, message }),
  discardAgentChanges: (agentId: string) => invoke<void>("discard_agent_changes", { agentId }),
  stashAgent: (agentId: string) => invoke<void>("stash_agent", { agentId }),
  abortMergeAgent: (agentId: string) => invoke<void>("abort_merge_agent", { agentId }),
  deleteBranchAgent: (agentId: string) => invoke<void>("delete_branch_agent", { agentId }),
  listRepoBranches: (repoPath: string) => invoke<string[]>("list_repo_branches", { repoPath }),
  createPr: (agentId: string, title: string, body: string) =>
    invoke<PrState>("create_pr", { agentId, title, body }),
  mergePr: (agentId: string) => invoke<void>("merge_pr", { agentId }),
  openAgentShell: (agentId: string) => invoke<void>("open_agent_shell", { agentId }),
  closeAgentShell: (agentId: string) => invoke<void>("close_agent_shell", { agentId }),
  writeToShell: (agentId: string, data: string) =>
    invoke<void>("write_to_shell", { agentId, data }),
  resizeShell: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_shell", { agentId, cols, rows }),
  runStart: (agentId: string) => invoke<void>("run_start", { agentId }),
  runStop: (agentId: string) => invoke<void>("run_stop", { agentId }),
  runState: (agentId: string) => invoke<RunStateSnapshot>("run_state", { agentId }),
  detectRunConfig: (agentId: string) => invoke<DetectedConfig[]>("detect_run_config", { agentId }),
  listWorktreeTree: (agentId: string) => invoke<WorktreeFile[]>("list_worktree_tree", { agentId }),
  listDir: (path: string) => invoke<DirListing>("list_dir", { path }),
  listPrs: (agentId: string) => invoke<PrSummary[]>("list_prs", { agentId }),
  readWorktreeFile: (agentId: string, path: string) =>
    invoke<WorktreeFileContents>("read_worktree_file", { agentId, path }),
  getFileDiff: (agentId: string, path: string) =>
    invoke<string>("get_file_diff", { agentId, path }),
  writeWorktreeFile: (agentId: string, path: string, contents: string) =>
    invoke<void>("write_worktree_file", { agentId, path, contents }),
  renameWorktreePath: (agentId: string, from: string, to: string) =>
    invoke<void>("rename_worktree_path", { agentId, from, to }),
  deleteWorktreePath: (agentId: string, path: string) =>
    invoke<void>("delete_worktree_path", { agentId, path }),
  createWorktreeFile: (agentId: string, path: string) =>
    invoke<void>("create_worktree_file", { agentId, path }),
  createWorktreeDir: (agentId: string, path: string) =>
    invoke<void>("create_worktree_dir", { agentId, path }),
  copyWorktreeFile: (agentId: string, from: string, to: string) =>
    invoke<void>("copy_worktree_file", { agentId, from, to }),
  probeProviderVersions: () => invoke<ProviderProbe[]>("probe_provider_versions"),
  /** Resolve a required non-agent CLI (e.g. "git") and probe its version. */
  checkCli: (name: string) => invoke<ToolStatus>("check_cli", { name }),
  /** Check a candidate custom binary path before saving it as an override. */
  validateAgentBin: (path: string) => invoke<BinValidation>("validate_agent_bin", { path }),
  /** Set (or clear, with a null/blank path) a per-agent custom binary path.
   *  Persists the setting and refreshes the backend's resolution registry. */
  setAgentBinOverride: (id: string, path: string | null) =>
    invoke<void>("set_agent_bin_override", { id, path }),
  /** Per-agent supported-model discovery (raw ids + any cheap CLI metadata).
   *  The frontend enriches these against models.dev. */
  discoverSupportedModels: () => invoke<AgentModels[]>("discover_supported_models"),
};

export function onAgentOutput(cb: (e: AgentOutputEvent) => void): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
}

export interface ShellOutputEvent {
  agent_id: string;
  /** Raw PTY bytes, base64-encoded over IPC. Decode with `decodeBase64`. */
  bytes: string;
}

export function onShellOutput(cb: (e: ShellOutputEvent) => void): Promise<UnlistenFn> {
  return listen<ShellOutputEvent>("shell:output", (event) => cb(event.payload));
}

export function onAgentEvent(cb: (e: AgentManagedEvent) => void): Promise<UnlistenFn> {
  return listen<AgentManagedEvent>("agent:event", (event) => cb(event.payload));
}

/** One canonical record from session_records: a verbatim per-provider
 *  transcript body plus its dedup key and provenance. */
export interface SessionRecord {
  seq: number;
  provider: string;
  source: string;
  native_id: string;
  agent_version: string | null;
  body: Record<string, unknown> & { type?: string };
}

/** One Fletch-origin outgoing user message (session_user_turns). Carries the
 *  attachment metadata the transcript lacks; `native_id` links it to the
 *  canonical session_records user-message once matched at turn-end (null =
 *  pending or failed — rendered standalone for retry). */
export interface UserTurn {
  turn_id: string;
  seq: number;
  text: string;
  attachments: string[];
  native_id: string | null;
  /** Epoch millis when the turn started running; null if it never started. */
  started_at: number | null;
  /** Epoch millis when the turn finished; null while in flight. */
  ended_at: number | null;
}

export interface SessionRecordsAppendedEvent {
  agent_id: string;
}

/** Fires when a turn's transcript has been ingested into session_records, so
 *  the canonical render can replace the ephemeral live one. */
export function onSessionRecordsAppended(
  cb: (e: SessionRecordsAppendedEvent) => void,
): Promise<UnlistenFn> {
  return listen<SessionRecordsAppendedEvent>("session:records-appended", (event) =>
    cb(event.payload),
  );
}

export interface TurnStartedEvent {
  agent_id: string;
  /** Backend epoch millis the turn began — the live-timer anchor. */
  started_at: number;
}

/** Fires when a turn flips to Running, carrying the backend's own start
 *  timestamp so the live timer shares the persisted duration's clock. */
export function onTurnStarted(cb: (e: TurnStartedEvent) => void): Promise<UnlistenFn> {
  return listen<TurnStartedEvent>("turn:started", (event) => cb(event.payload));
}

export function onAgentStatus(cb: (e: AgentStatusEvent) => void): Promise<UnlistenFn> {
  return listen<AgentStatusEvent>("agent:status", (event) => cb(event.payload));
}

export function onAgentView(cb: (e: AgentViewEvent) => void): Promise<UnlistenFn> {
  return listen<AgentViewEvent>("agent:view", (event) => cb(event.payload));
}

export function onAgentTask(cb: (e: AgentTaskEvent) => void): Promise<UnlistenFn> {
  return listen<AgentTaskEvent>("agent:task", (event) => cb(event.payload));
}

export function onAgentBranch(cb: (e: AgentBranchEvent) => void): Promise<UnlistenFn> {
  return listen<AgentBranchEvent>("agent:branch", (event) => cb(event.payload));
}

export function onAgentRepoAdded(cb: (e: AgentRepoAddedEvent) => void): Promise<UnlistenFn> {
  return listen<AgentRepoAddedEvent>("agent:repo_added", (event) => cb(event.payload));
}

export function onAgentGitAction(cb: (e: AgentGitActionEvent) => void): Promise<UnlistenFn> {
  return listen<AgentGitActionEvent>("agent:git-action", (event) => cb(event.payload));
}

export function onWorkspaceChanged(cb: () => void): Promise<UnlistenFn> {
  return listen<unknown>("workspace:changed", () => cb());
}

export function onPrStateChanged(cb: (e: PrStateChangedEvent) => void): Promise<UnlistenFn> {
  return listen<PrStateChangedEvent>("pr:state_changed", (event) => cb(event.payload));
}

export function onRunOutput(cb: (e: RunOutputEvent) => void): Promise<UnlistenFn> {
  return listen<RunOutputEvent>("run:output", (event) => cb(event.payload));
}

export function onRunState(cb: (e: RunStateEvent) => void): Promise<UnlistenFn> {
  return listen<RunStateEvent>("run:state", (event) => cb(event.payload));
}
