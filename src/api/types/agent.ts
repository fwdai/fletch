import type { DiffStats } from "./git";

export type AgentStatus = "spawning" | "running" | "idle" | "stopped" | "error";

export type AgentView = "custom" | "native";

/** What a forked workspace's worktree starts from. `clean` forks the parent's
 *  base branch; `carry` overlays the parent's current working tree (incl.
 *  uncommitted work) so the fork builds on unmerged work. Mirrors the backend
 *  `ForkCode`. */
export type ForkCode = "clean" | "carry";

/** How much of the parent conversation a fork carries. Mirrors the backend
 *  `ForkContext`. Summarized context ships in a follow-up slice. */
export type ForkContext =
  | { kind: "none" }
  | { kind: "full" }
  | { kind: "up_to_message"; prompt: number };

export interface TrackedRepo {
  repo_path: string;
  subdir: string;
  branch?: string | null;
  parent_branch?: string | null;
  /** Bound PR identity + last persisted snapshot (see prSnapshot in
   *  util/prState.ts). Written by the backend on every successful PR fetch;
   *  what the UI renders when GitHub or the checkout is unavailable. */
  pr_number?: number | null;
  pr_url?: string | null;
  pr_title?: string | null;
  pr_state?: string | null;
  /** The repo's display label within its project ("Frontend"); null falls
   *  back to the folder basename. Denormalized from the repos table. */
  label?: string | null;
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
  /** Repos this agent has checkouts in. Always non-empty;
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
  /** Sandbox engine stamped at creation ("sandbox-exec" | "docker") and kept
   *  for the agent's life — a settings change never re-engines it. Null for
   *  agents created before engine selection existed (they run sandbox-exec). */
  sandbox_engine?: string | null;
  /** The GitHub issue this workspace was started from (bare issue number as
   *  text), set by the Home inbox's "Start work". Null for a normal spawn. */
  issue_ref?: string | null;
}

/** A pinned repo joined with its owning project. `name` is the project's
 *  user-editable display name (defaults to the folder basename, survives
 *  rename/relocate); `project_id` addresses the project for rename/relocate +
 *  per-project settings. `label` names this repo within a multi-repo project
 *  ("Frontend", "Gateway"); null falls back to the folder basename. */
export interface ProjectRef {
  path: string;
  name: string;
  project_id: string;
  label: string | null;
}

export interface Workspace {
  /** Repos pinned in the sidebar. Empty on first launch. */
  repos: string[];
  /** Per-repo project metadata, parallel to `repos`. */
  projects: ProjectRef[];
  agents: AgentRecord[];
}

export interface ProjectDeleteResult {
  workspace: Workspace;
  deleted_agent_ids: string[];
  deleted_run_ids: string[];
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

export interface ShellOutputEvent {
  agent_id: string;
  /** Raw PTY bytes, base64-encoded over IPC. Decode with `decodeBase64`. */
  bytes: string;
}
