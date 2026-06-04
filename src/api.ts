import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type AgentStatus =
  | "spawning"
  | "running"
  | "idle"
  | "stopped"
  | "error";

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
}

export interface Workspace {
  /** Repos pinned in the sidebar. Empty on first launch. */
  repos: string[];
  agents: AgentRecord[];
}

export interface AgentOutputEvent {
  agent_id: string;
  bytes: number[];
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

export type StatusKind =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted";

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

export type RunPhase = "idle" | "setup" | "running" | "stopped";

export interface RunStateSnapshot {
  phase: RunPhase;
  last_error: string | null;
  /** Raw PTY bytes accumulated since the panel was last cleared.
   *  Sent as a JSON array of u8 values; decode with TextDecoder. */
  log: number[];
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

export const api = {
  getWorkspace: () => invoke<Workspace | null>("get_workspace"),
  getAgentDiffStats: (agentId: string) =>
    invoke<DiffStats>("get_agent_diff_stats", { agentId }),
  addWorkspaceRepo: (repoPath: string) =>
    invoke<Workspace>("add_workspace_repo", { repoPath }),
  removeWorkspaceRepo: (repoPath: string) =>
    invoke<Workspace>("remove_workspace_repo", { repoPath }),
  spawnAgent: (view: AgentView, repoPath: string, provider?: string) =>
    invoke<AgentRecord>("spawn_agent", { view, repoPath, provider }),
  writeToAgent: (agentId: string, data: string) =>
    invoke<void>("write_to_agent", { agentId, data }),
  sendUserMessage: (agentId: string, text: string, attachments: string[] = []) =>
    invoke<void>("send_user_message", { agentId, text, attachments }),
  resizeAgent: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_agent", { agentId, cols, rows }),
  switchView: (agentId: string, view: AgentView) =>
    invoke<void>("switch_view", { agentId, view }),
  resumeAgent: (agentId: string) =>
    invoke<void>("resume_agent", { agentId }),
  stopAgent: (agentId: string) => invoke<void>("stop_agent", { agentId }),
  discardAgent: (agentId: string) =>
    invoke<void>("discard_agent", { agentId }),
  archiveAgent: (agentId: string) =>
    invoke<void>("archive_agent", { agentId }),
  restoreAgent: (agentId: string) =>
    invoke<void>("restore_agent", { agentId }),
  readSessionTranscript: (agentId: string) =>
    invoke<Array<Record<string, unknown> & { type?: string }>>(
      "read_session_transcript",
      { agentId },
    ),
  readSessionEvents: (agentId: string) =>
    invoke<Array<Record<string, unknown> & { type?: string }>>(
      "read_session_events",
      { agentId },
    ),
  addRepoToAgent: (agentId: string, repoPath: string) =>
    invoke<TrackedRepo>("add_repo_to_agent", { agentId, repoPath }),
  allocateDraftName: (used: string[]) =>
    invoke<string>("allocate_draft_name", { used }),
  getGitState: (agentId: string) =>
    invoke<GitState | null>("get_git_state", { agentId }),
  getAllShortstats: () =>
    invoke<Record<string, ShortStats>>("get_all_shortstats"),
  getPrState: (agentId: string) =>
    invoke<PrState | null>("get_pr_state", { agentId }),
  pushAgent: (agentId: string) => invoke<string>("push_agent", { agentId }),
  pullAgent: (agentId: string) => invoke<void>("pull_agent", { agentId }),
  rebaseAgent: (agentId: string) => invoke<void>("rebase_agent", { agentId }),
  commitAgent: (agentId: string, message: string) =>
    invoke<void>("commit_agent", { agentId, message }),
  discardAgentChanges: (agentId: string) =>
    invoke<void>("discard_agent_changes", { agentId }),
  stashAgent: (agentId: string) => invoke<void>("stash_agent", { agentId }),
  abortMergeAgent: (agentId: string) =>
    invoke<void>("abort_merge_agent", { agentId }),
  deleteBranchAgent: (agentId: string) =>
    invoke<void>("delete_branch_agent", { agentId }),
  createPr: (agentId: string, title: string, body: string) =>
    invoke<PrState>("create_pr", { agentId, title, body }),
  mergePr: (agentId: string) => invoke<void>("merge_pr", { agentId }),
  openAgentShell: (agentId: string) =>
    invoke<void>("open_agent_shell", { agentId }),
  closeAgentShell: (agentId: string) =>
    invoke<void>("close_agent_shell", { agentId }),
  writeToShell: (agentId: string, data: string) =>
    invoke<void>("write_to_shell", { agentId, data }),
  resizeShell: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_shell", { agentId, cols, rows }),
  runStart: (agentId: string) => invoke<void>("run_start", { agentId }),
  runStop: (agentId: string) => invoke<void>("run_stop", { agentId }),
  runState: (agentId: string) =>
    invoke<RunStateSnapshot>("run_state", { agentId }),
  listWorktreeTree: (agentId: string) =>
    invoke<WorktreeFile[]>("list_worktree_tree", { agentId }),
  readWorktreeFile: (agentId: string, path: string) =>
    invoke<WorktreeFileContents>("read_worktree_file", { agentId, path }),
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
};

export function onAgentOutput(
  cb: (e: AgentOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
}

export interface ShellOutputEvent {
  agent_id: string;
  bytes: number[];
}

export function onShellOutput(
  cb: (e: ShellOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<ShellOutputEvent>("shell:output", (event) => cb(event.payload));
}

export function onAgentEvent(
  cb: (e: AgentManagedEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentManagedEvent>("agent:event", (event) => cb(event.payload));
}

export function onAgentStatus(
  cb: (e: AgentStatusEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentStatusEvent>("agent:status", (event) => cb(event.payload));
}

export function onAgentView(
  cb: (e: AgentViewEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentViewEvent>("agent:view", (event) => cb(event.payload));
}

export function onAgentTask(
  cb: (e: AgentTaskEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentTaskEvent>("agent:task", (event) => cb(event.payload));
}

export function onAgentBranch(
  cb: (e: AgentBranchEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentBranchEvent>("agent:branch", (event) =>
    cb(event.payload),
  );
}

export function onAgentRepoAdded(
  cb: (e: AgentRepoAddedEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentRepoAddedEvent>("agent:repo_added", (event) =>
    cb(event.payload),
  );
}

export function onWorkspaceChanged(cb: () => void): Promise<UnlistenFn> {
  return listen<unknown>("workspace:changed", () => cb());
}

export function onPrStateChanged(
  cb: (e: PrStateChangedEvent) => void,
): Promise<UnlistenFn> {
  return listen<PrStateChangedEvent>("pr:state_changed", (event) => cb(event.payload));
}

export function onRunOutput(
  cb: (e: RunOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<RunOutputEvent>("run:output", (event) => cb(event.payload));
}

export function onRunState(
  cb: (e: RunStateEvent) => void,
): Promise<UnlistenFn> {
  return listen<RunStateEvent>("run:state", (event) => cb(event.payload));
}
