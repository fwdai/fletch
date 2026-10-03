import type { DiffStats } from "./git";

export type AgentStatus = "spawning" | "running" | "idle" | "stopped" | "error";

export type AgentView = "custom" | "native";

/** What a forked workspace's checkouts start from: `clean` — the parent's base
 *  branch; `current` — the parent's code now; `at_message` — the code as the
 *  anchor message's reply left it (message forks only). The last two copy the
 *  code faithfully, commits as commits and uncommitted work as uncommitted.
 *  Mirrors the backend `ForkCode`. */
export type ForkCode = "clean" | "current" | "at_message";

/** How a forked session's agent knows the parent conversation up to the
 *  fork's anchor: `full` — it resumes the whole conversation natively, as its
 *  own transcript (providers with a transcript writer only); `summary` — it is
 *  told a summary of it. Either way the chat shows that history. Mirrors the
 *  backend `ForkContext`. */
export type ForkContext = "full" | "summary";

/** What a rewind to just before a message puts back. Mirrors the backend
 *  `RewindScope`. */
export type RewindScope = "conversation" | "code" | "both";

/** A commit a code restore takes off its checkout's branch. `pushed`: origin
 *  has it, so the branch's next push has to force. */
export interface LeavingCommit {
  sha: string;
  subject: string;
  pushed: boolean;
}

/** One checkout in a `RestoreReport`. */
export interface RepoRestore {
  subdir: string;
  /** The branch that goes back with HEAD; null when HEAD is detached. */
  branch: string | null;
  /** The message's checkpoint; null leaves this checkout as it is. */
  checkpoint: string | null;
  /** Newest first. */
  leaving: LeavingCommit[];
}

/** What restoring the code as of a message does to each of an agent's
 *  checkouts: previewed before, and reported after. The undo point a restore
 *  leaves is the backend's (`undoCodeRestore`). Mirrors the backend
 *  `RestoreReport`. */
export interface RestoreReport {
  repos: RepoRestore[];
}

/** What a rewind did. `conversation_error` says why the conversation couldn't
 *  follow a code restore that already happened, which can still be undone. */
export interface RewindOutcome {
  code: RestoreReport | null;
  conversation_error: string | null;
}

/** Where an agent's session branches off an earlier one: the parent session
 *  and the exclusive record seq below which it shows the parent's history.
 *  Mirrors the backend `SessionLineage`. */
export interface SessionLineage {
  parent_session_id: string;
  cut_seq: number;
}

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
  /** Set when the agent's session continues an earlier conversation (a fork),
   *  whose history it shows before its own. */
  lineage?: SessionLineage | null;
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
  /** What this workspace is for, when a surface other than the sidebar owns it
   *  (see [`ROADMAP_PM_PURPOSE`]). Null — the normal case — is a sidebar agent.
   *  Tagged workspaces are absent from the `get_workspace` snapshot entirely;
   *  they arrive through `listProjectChats`. */
  purpose?: string | null;
  /** A short title for the work, written by the agent via the `set_title`
   *  mailbox op once it knows what the user wants. Null until then; the
   *  sidebar subtitle falls back to the first line of `task`. */
  title?: string | null;
  /** The workflow run that owns this agent, when it is a run's step agent.
   *  Null for a user's agent. */
  owner_run_id?: string | null;
}

/** `workspaces.purpose` for a Roadmap project-manager chat — a manual chat that
 *  lives only on a project's Roadmap tab, never in the sidebar (which is for
 *  feature-development agents and workflow runs). Mirrors the backend's
 *  `workspace::PURPOSE_ROADMAP_PM`; the backend also denies these workspaces the
 *  publish ops, so a PM chat can read the code but never ship it. */
export const ROADMAP_PM_PURPOSE = "roadmap-pm";

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

/** One auto-archive pass that moved something to History
 *  (`workspace:auto-archived`). `workspace:changed` already reloaded the
 *  sidebar; this names what went so the user can be told. */
export interface WorkspaceAutoArchivedEvent {
  agent_ids: string[];
  names: string[];
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
  /** The agent's per-event count under the current host process, matching
   *  `LiveTurn.next_seq`: a client that replayed the running turn skips frames
   *  numbered below the snapshot it folded. Absent from hosts before it. */
  seq?: number;
}

export interface AgentStatusEvent {
  agent_id: string;
  status: AgentStatus;
  last_error: string | null;
}

/** Which step of a fresh spawn is running, from `agent:spawn-progress`. A host
 *  newer than this client can name a stage that is not here, so readers must
 *  tolerate an unknown value rather than index blindly. */
export type SpawnStage =
  | "preparing"
  | "cloning"
  | "indexing"
  | "attaching_repos"
  | "carrying"
  | "summarizing"
  | "starting";

/** Progress behind the `spawning` status — a label for the wait, never
 *  authoritative state. Not snapshotted: a client that connects mid-spawn has
 *  no stage until the next one fires. */
export interface AgentSpawnProgressEvent {
  agent_id: string;
  stage: SpawnStage;
  /** What the stage is working on, when it has something to name (the repo
   *  being checked out, say). */
  detail: string | null;
}

export interface AgentViewEvent {
  agent_id: string;
  view: AgentView;
}

export interface AgentEffortEvent {
  agent_id: string;
  effort: string | null;
}

export interface AgentModelEvent {
  agent_id: string;
  model: string | null;
}

export interface AgentTaskEvent {
  agent_id: string;
  task: string;
}

export interface AgentTitleEvent {
  agent_id: string;
  title: string;
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
