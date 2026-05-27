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

export interface GitState {
  branch: string;
  parent_branch: string;
  ahead: number;
  behind: number;
  files: FileStatus[];
  additions: number;
  deletions: number;
}

export interface GitStateChangedEvent {
  agent_id: string;
  state: GitState;
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
  sendUserMessage: (agentId: string, text: string) =>
    invoke<void>("send_user_message", { agentId, text }),
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
  addRepoToAgent: (agentId: string, repoPath: string) =>
    invoke<TrackedRepo>("add_repo_to_agent", { agentId, repoPath }),
  allocateDraftName: (used: string[]) =>
    invoke<string>("allocate_draft_name", { used }),
  getGitState: (agentId: string) =>
    invoke<GitState | null>("get_git_state", { agentId }),
};

export function onAgentOutput(
  cb: (e: AgentOutputEvent) => void,
): Promise<UnlistenFn> {
  return listen<AgentOutputEvent>("agent:output", (event) => cb(event.payload));
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

export function onGitStateChanged(
  cb: (e: GitStateChangedEvent) => void,
): Promise<UnlistenFn> {
  return listen<GitStateChangedEvent>("git:state_changed", (event) => cb(event.payload));
}
