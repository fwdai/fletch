import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentModels } from "./data/modelCatalog/types";
import type { McpServerSnapshot } from "./storage/mcpServers";
import type { SandboxEngine } from "./storage/preferences";
import type { SkillSnapshot } from "./storage/skills";
import type { Budgets, Definition, ImportReport, Spec } from "./workflows/spec";

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

/** Advisory fleet-wide git metadata for one checkout, keyed by `gitKey` in the
 *  bulk `getAllGitMeta` reply. `behind` (base moved ahead of this checkout) and
 *  `files` (working-tree paths) drive the always-visible staleness chip and the
 *  cross-agent overlap hints. `behind` is null when the base tip can't be
 *  resolved (no GitHub / no fetch yet) — render nothing, never a zero. */
export interface GitMeta {
  base: string;
  behind: number | null;
  files: string[];
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

/** One file in the checkout, as returned by `list_checkout_tree`.
 *  `status` is the single-letter git status vs the parent branch
 *  ("M" | "A" | "D" | "R"), or null when the file is unchanged.
 *  Multi-repo agents get paths prefixed with the owning checkout's subdir
 *  ("<subdir>/<rel>"); the file read/write commands resolve the same prefix
 *  back, so the panel can pass these paths through unchanged. */
export interface CheckoutFile {
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

/** A user- or project-level slash command found on disk by
 *  `discover_slash_commands` (e.g. a `~/.claude/commands/*.md`). Mirrors the
 *  Rust `DiscoveredCommand`; always maps to a `passthrough` command in the
 *  composer. `scope` is "user" or "project" ("project" shadows "user"). */
export interface DiscoveredCommand {
  name: string;
  description: string;
  hint?: string;
  scope: "user" | "project";
}

/** Captured output of a one-shot `claude <args>` invocation run for a local
 *  slash command (e.g. `/doctor`). Mirrors the Rust `ClaudeCommandOutput`. */
export interface ClaudeCommandOutput {
  stdout: string;
  stderr: string;
  success: boolean;
}

/** A checkout file's contents plus the metadata the File-panel editor
 *  needs. `chg_add` / `chg_mod` are 1-indexed line numbers the agent
 *  added / modified (drives the change gutter). */
export interface CheckoutFileContents {
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

/** One label on an issue, for the Home inbox's quiet chips. `color` is
 *  GitHub's 6-hex assignment (no leading `#`), used subtly when present. */
export interface IssueLabel {
  name: string;
  color?: string;
}

/** An open GitHub issue for the Home inbox. Carries the body so "Start work"
 *  composes the brief without a second round-trip. */
export interface IssueSummary {
  number: number;
  title: string;
  url: string;
  labels: IssueLabel[];
  assignee?: string;
  /** `updatedAt` as ms-epoch, for the "updated N ago" hint. */
  updated_at?: number;
  body?: string;
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
  /** "version" | "install" | "dev" | "test" | "build" | "lint" | "port" | "env" */
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

/** One check's terminal outcome in a verification run (Rust
 *  `verify::CheckOutcome`). `skipped` = no command resolved (nothing to run);
 *  `setup_failed` = a prerequisite install failed so the check never ran. */
export type CheckOutcome = "passed" | "failed" | "timed_out" | "setup_failed" | "skipped";

/** One check's result inside a verification run (Rust `verify::CheckResult`). */
export interface CheckResult {
  /** "install" | "test" | "lint" */
  name: string;
  /** The command that ran (or would have); "" when skipped. */
  command: string;
  outcome: CheckOutcome;
  /** Wall-clock duration in ms; 0 when the check didn't run. */
  duration_ms: number;
  /** Last ~100 lines of combined stdout+stderr; empty on success/skip. */
  tail: string[];
}

/** The result of running a project's deterministic checks in a checkout
 *  (Rust `verify::VerificationReport`). */
export interface VerificationReport {
  checks: CheckResult[];
}

/** One changed file in an approval gate's ferried diff. */
export interface GateDiffFile {
  path: string;
  additions: number;
  deletions: number;
}

/** The ferried diff (vs the run base) summarized for review. */
export interface GateDiff {
  additions: number;
  deletions: number;
  files: GateDiffFile[];
}

/** Budget spent vs cap at an approval pause. `tokens_cap === null` means the run
 *  has no token cap; a `tokens_spent` of 0 with no cap should render as "unknown"
 *  (some providers don't report token usage — driver.rs). */
export interface GateBudget {
  turns_spent: number;
  turns_cap: number;
  tokens_spent: number;
  tokens_cap: number | null;
  wall_ms_spent: number;
  wall_clock_cap_mins: number;
}

/** A reviewer step's `verdict.json` summary carried in the evidence. */
export interface GateVerdict {
  result: string;
  summary: string;
  detail: string | null;
  target: string | null;
}

/** The review evidence assembled when an approval gate pauses a run (the Rust
 *  `gate_evidence` event payload, spec §9): verification, the ferried diff vs the
 *  run base, budget spend, and the step's verdict. `verification` is null when the
 *  host couldn't build a verifier; its checks are all `skipped` when the project
 *  configures no commands. `base_sha`/`head_sha` feed `api.wfRunDiff`. */
export interface GateEvidence {
  base_sha: string;
  head_sha: string;
  verification: VerificationReport | null;
  diff: GateDiff;
  budget: GateBudget;
  verdict: GateVerdict | null;
}

/** Project-scoped run config resolved from a repo path: the detected configs
 *  plus the project_id they belong to (see Rust `supervisor::ProjectRunConfig`). */
export interface ProjectRunConfig {
  project_id: string;
  configs: DetectedConfig[];
}

/** One `KEY=value` pair discovered in a project's `.env` (Rust
 *  `run_env::EnvEntry`). Used by the Run & Environment settings list. */
export interface EnvEntry {
  key: string;
  value: string;
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

/** The port the dev server is actually launching on — emitted just before the
 *  run phase spawns. May differ from the configured port when port-safety
 *  bumped it to the next free one. */
export interface RunPortEvent {
  agent_id: string;
  port: number;
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

/** Result of probing the local Docker installation (Settings › General).
 *  `version` is the daemon's server version, present only when available. */
export interface DockerProbe {
  status: "available" | "not-installed" | "daemon-down";
  version?: string;
}

/** Which step of the container auth chain (pasted token → shell env →
 *  claude credentials file) would supply Anthropic credentials to a docker
 *  agent right now (Settings › General › Sandbox status row). */
export interface ContainerAuthStatus {
  status: "keychain" | "stored-token" | "shell-env" | "credentials-file" | "none";
}

/** One image-build lifecycle event from the `docker:build-progress` stream.
 *  The embedded agent image is built on the first docker spawn (a slow
 *  `docker build`); these feed the build toast. `line` is set only on `"line"`,
 *  `error` only on `"failed"`. */
export interface DockerBuildEvent {
  phase: "started" | "line" | "finished" | "failed";
  line?: string;
  error?: string;
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
  /** Open an agent's checkout in the chosen editor. */
  openInEditor: (agentId: string, editorId: string) =>
    invoke<void>("open_in_editor", { agentId, editorId }),
  // Anonymous usage telemetry. Persists the opt-out flag and toggles the live
  // pipeline (events themselves are emitted from the backend).
  setTelemetryEnabled: (enabled: boolean) => invoke<void>("set_telemetry_enabled", { enabled }),
  // Emit the deferred first `app_opened` once onboarding completes — i.e. after
  // the data-sharing disclosure has been shown. See `track_app_opened` (Rust).
  trackAppOpened: () => invoke<void>("track_app_opened"),
  // Sandbox engine selection. The setting is backend-owned (snake_case
  // `sandbox_engine`, written by `set_sandbox_engine` — which validates docker
  // against a live daemon probe and refuses when it's unreachable).
  getSandboxEngine: () => invoke<SandboxEngine>("get_sandbox_engine"),
  setSandboxEngine: (engine: SandboxEngine) => invoke<void>("set_sandbox_engine", { engine }),
  probeDockerEngine: () => invoke<DockerProbe>("probe_docker_engine"),
  // Anthropic auth for containerized agents. Docker-only: seatbelt agents keep
  // the user's own claude login. The token is backend-owned
  // (`claude_container_token` in the backend secret store, written by the
  // set/clear commands below — never via a frontend `setSetting`).
  getContainerAuthStatus: () => invoke<ContainerAuthStatus>("get_container_auth_status"),
  setContainerAuthToken: (token: string) => invoke<void>("set_container_auth_token", { token }),
  clearContainerAuthToken: () => invoke<void>("clear_container_auth_token"),
  // Automated `claude setup-token` capture: drives the CLI under a PTY,
  // surfaces the consent URL + auth-code prompt as `claude-setup:url` /
  // `claude-setup:awaiting-code` events, and resolves once the token is stored.
  // The token itself never crosses this boundary. See `useClaudeSetup`.
  connectClaudeContainerAuth: () => invoke<void>("connect_claude_container_auth"),
  submitClaudeSetupCode: (code: string) => invoke<void>("submit_claude_setup_code", { code }),
  cancelClaudeContainerAuth: () => invoke<void>("cancel_claude_container_auth"),
  // Advanced docker launch knobs (image override + resource limits). Backend-
  // owned settings (`docker_image` / `docker_memory` / `docker_cpus`): the
  // command persists all three AND updates the spawn-path mirror. Blank clears
  // a field (falls back to the launch defaults). Never write these via a
  // frontend `setSetting`.
  setDockerLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_docker_launch_settings", { image, memory, cpus }),
  /** Launch Docker Desktop (the daemon-down error state's action). macOS-only;
   *  rejects elsewhere. */
  startDockerDesktop: () => invoke<void>("start_docker_desktop"),
  getAgentDiffStats: (agentId: string) => invoke<DiffStats>("get_agent_diff_stats", { agentId }),
  /** The current HEAD commit SHA of an agent's checkout (primary repo). The
   *  fork point for "promote to workflow". */
  agentHeadSha: (agentId: string) => invoke<string>("agent_head_sha", { agentId }),
  addWorkspaceRepo: (repoPath: string) => invoke<Workspace>("add_workspace_repo", { repoPath }),
  removeWorkspaceRepo: (repoPath: string) =>
    invoke<Workspace>("remove_workspace_repo", { repoPath }),
  /** Attach a repo to an existing project (multi-repo projects). */
  attachRepoToProject: (projectId: string, repoPath: string) =>
    invoke<Workspace>("attach_repo_to_project", { projectId, repoPath }),
  /** Detach a repo from a project. Rejects the last repo and repos still
   *  referenced by agent checkouts (live or archived). */
  detachRepoFromProject: (projectId: string, repoPath: string) =>
    invoke<Workspace>("detach_repo_from_project", { projectId, repoPath }),
  /** Set a repo's display label within its project. Blank clears back to the
   *  folder-basename fallback. */
  setRepoLabel: (repoPath: string, label: string) =>
    invoke<Workspace>("set_repo_label", { repoPath, label }),
  /** Set a project's custom display name (independent of its folder). */
  renameProject: (projectId: string, name: string) =>
    invoke<Workspace>("rename_project", { projectId, name }),
  /** Delete a project and all of its agents/workspaces. Active agents block it. */
  deleteProject: (projectId: string) =>
    invoke<ProjectDeleteResult>("delete_project", { projectId }),
  projectHasRunningAgents: (projectId: string) =>
    invoke<boolean>("project_has_running_agents", { projectId }),
  /** Repoint a pinned repo at a moved folder. Rejects a non-git or
   *  already-pinned destination. */
  relocateRepo: (oldPath: string, newPath: string) =>
    invoke<Workspace>("relocate_repo", { oldPath, newPath }),
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
    /** Base the checkout forks from and the agent's recorded parent branch
     *  (PR base / ahead-behind). The new-agent screen passes the chosen base
     *  branch; a workflow step instead passes the previous step's HEAD
     *  (commit-ish) so its checkout continues that work. */
    forkBase?: string,
    /** A custom agent's skills, resolved by value at spawn (snapshotted onto
     *  the session like `instructions`). */
    skills?: SkillSnapshot[],
    /** A custom agent's MCP servers, resolved by value at spawn. */
    mcpServers?: McpServerSnapshot[],
    /** The GitHub issue this spawn originates from (bare issue number as text),
     *  set by the Home inbox's "Start work". Persisted so the agent's PR closes
     *  it. `undefined` for a spawn not tied to an issue. */
    issueRef?: string,
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
      skills: skills ?? null,
      mcpServers: mcpServers ?? null,
      forkBase: forkBase ?? null,
      issueRef: issueRef ?? null,
    }),
  /** Fork an existing workspace into a new one, seeding its worktree (`code`)
   *  and conversation (`context`) independently. For `context.kind ===
   *  "up_to_message"`, `prompt` is the 0-based ordinal of a navigable user
   *  prompt (git-action turns excluded), matching the chat's turn list.
   *
   *  `contextDigest` is the rendered prose for the carried range, assembled by
   *  the caller from the normalized chat log (so it works uniformly across every
   *  provider and matches the history the child shows). `null` when nothing is
   *  carried.
   *
   *  `snapshotMaxSeq` is the highest `session_records.seq` the caller saw when it
   *  built the digest. The backend caps its own (possibly newer) record read at
   *  this seq before copying, so a sync that appends to the parent between the
   *  two reads can never seed the child with turns the digest omitted. `null`
   *  when nothing is carried (or the caller saw no records). */
  forkAgent: (
    parentId: string,
    code: ForkCode,
    context: ForkContext,
    contextDigest: string | null,
    snapshotMaxSeq: number | null,
  ) =>
    invoke<AgentRecord>("fork_agent", {
      parentId,
      code,
      context,
      contextDigest,
      snapshotMaxSeq,
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
  // Git/PR commands below take an optional `subdir` (a checkout's directory
  // name from `TrackedRepo.subdir`) to target one repo of a multi-repo agent.
  // Omitted/undefined serializes to None = the agent's primary (first) repo.
  getGitState: (agentId: string, subdir?: string) =>
    invoke<GitState | null>("get_git_state", { agentId, subdir }),
  getAllShortstats: () => invoke<Record<string, ShortStats>>("get_all_shortstats"),
  getAllGitMeta: () => invoke<Record<string, GitMeta>>("get_all_git_meta"),
  refreshBaseFreshness: () => invoke<void>("refresh_base_freshness"),
  getPrState: (agentId: string, subdir?: string) =>
    invoke<PrState | null>("get_pr_state", { agentId, subdir }),
  refreshAllPrStates: () => invoke<Record<string, PrState | null>>("refresh_all_pr_states"),
  refreshAllPrChecks: () => invoke<Record<string, PrChecks | null>>("refresh_all_pr_checks"),
  getPrChecks: (agentId: string, subdir?: string) =>
    invoke<PrChecks | null>("get_pr_checks", { agentId, subdir }),
  getPrComments: (agentId: string, subdir?: string) =>
    invoke<PrComments | null>("get_pr_comments", { agentId, subdir }),
  pushAgent: (agentId: string, subdir?: string) =>
    invoke<string>("push_agent", { agentId, subdir }),
  pullAgent: (agentId: string, subdir?: string) => invoke<void>("pull_agent", { agentId, subdir }),
  rebaseAgent: (agentId: string, subdir?: string) =>
    invoke<void>("rebase_agent", { agentId, subdir }),
  commitAgent: (agentId: string, message: string, subdir?: string) =>
    invoke<void>("commit_agent", { agentId, message, subdir }),
  discardAgentChanges: (agentId: string, subdir?: string) =>
    invoke<void>("discard_agent_changes", { agentId, subdir }),
  stashAgent: (agentId: string, subdir?: string) =>
    invoke<void>("stash_agent", { agentId, subdir }),
  abortMergeAgent: (agentId: string, subdir?: string) =>
    invoke<void>("abort_merge_agent", { agentId, subdir }),
  deleteBranchAgent: (agentId: string, subdir?: string) =>
    invoke<void>("delete_branch_agent", { agentId, subdir }),
  listRepoBranches: (repoPath: string) => invoke<string[]>("list_repo_branches", { repoPath }),
  createPr: (agentId: string, title: string, body: string, subdir?: string) =>
    invoke<PrState>("create_pr", { agentId, title, body, subdir }),
  mergePr: (agentId: string, subdir?: string) => invoke<void>("merge_pr", { agentId, subdir }),
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
  /** Run the project's deterministic checks (install → test → lint) in an
   *  agent's checkout. `subdir` targets a specific tracked repo (primary when
   *  omitted). */
  runVerification: (agentId: string, subdir?: string) =>
    invoke<VerificationReport>("run_verification", { agentId, subdir: subdir ?? null }),
  projectRunConfig: (repoPath: string) =>
    invoke<ProjectRunConfig>("project_run_config", { repoPath }),
  readEnvFileKeys: (repoPath: string) => invoke<EnvEntry[]>("read_env_file_keys", { repoPath }),
  getEnvOverride: (projectId: string, key: string) =>
    invoke<string | null>("get_env_override", { projectId, key }),
  setEnvOverride: (projectId: string, key: string, value: string) =>
    invoke<void>("set_env_override", { projectId, key, value }),
  clearEnvOverride: (projectId: string, key: string) =>
    invoke<void>("clear_env_override", { projectId, key }),
  listCheckoutTree: (agentId: string) => invoke<CheckoutFile[]>("list_checkout_tree", { agentId }),
  listDir: (path: string) => invoke<DirListing>("list_dir", { path }),
  discoverSlashCommands: (provider: string, projectDir?: string) =>
    invoke<DiscoveredCommand[]>("discover_slash_commands", {
      provider,
      projectDir: projectDir ?? null,
    }),
  runClaudeCommand: (agentId: string, args: string[]) =>
    invoke<ClaudeCommandOutput>("run_claude_command", { agentId, args }),
  listPrs: (agentId: string) => invoke<PrSummary[]>("list_prs", { agentId }),
  // Draft (new-workspace) composer variants, keyed by repo path since a draft
  // has no agent/checkout yet.
  listRepoTree: (repoPath: string) => invoke<string[]>("list_repo_tree", { repoPath }),
  listRepoPrs: (repoPath: string) => invoke<PrSummary[]>("list_repo_prs", { repoPath }),
  /** Open GitHub issues for the Home inbox, by repo path. `null` when the repo
   *  has no token / non-GitHub origin / a rate-limit pause is active — the
   *  section degrades quietly. `[]` means connected but no open issues. */
  listRepoIssues: (repoPath: string) =>
    invoke<IssueSummary[] | null>("list_repo_issues", { repoPath }),
  readCheckoutFile: (agentId: string, path: string) =>
    invoke<CheckoutFileContents>("read_checkout_file", { agentId, path }),
  getFileDiff: (agentId: string, path: string) =>
    invoke<string>("get_file_diff", { agentId, path }),
  writeCheckoutFile: (agentId: string, path: string, contents: string) =>
    invoke<void>("write_checkout_file", { agentId, path, contents }),
  renameCheckoutPath: (agentId: string, from: string, to: string) =>
    invoke<void>("rename_checkout_path", { agentId, from, to }),
  deleteCheckoutPath: (agentId: string, path: string) =>
    invoke<void>("delete_checkout_path", { agentId, path }),
  createCheckoutFile: (agentId: string, path: string) =>
    invoke<void>("create_checkout_file", { agentId, path }),
  createCheckoutDir: (agentId: string, path: string) =>
    invoke<void>("create_checkout_dir", { agentId, path }),
  copyCheckoutFile: (agentId: string, from: string, to: string) =>
    invoke<void>("copy_checkout_file", { agentId, from, to }),
  probeProviderVersions: () => invoke<ProviderProbe[]>("probe_provider_versions"),
  /** Resolve a required non-agent CLI (e.g. "git") and probe its version. */
  checkCli: (name: string) => invoke<ToolStatus>("check_cli", { name }),
  /** Manually (re)run portable-git resolution/installation — the retry for a
   *  failed startup bootstrap. Progress arrives via `git-dist:state` events
   *  (see useGitDist); rejects with the install error on failure. */
  gitDistInstall: () => invoke<void>("git_dist_install"),
  /** Run the pinned official installer for an agent CLI. Progress arrives via
   *  `agent-install:state` events; resolves when the installer exits. Callers
   *  re-probe providers afterwards to confirm detection. */
  installAgent: (id: string) => invoke<void>("install_agent", { id }),
  /** Check a candidate custom binary path before saving it as an override. */
  validateAgentBin: (path: string) => invoke<BinValidation>("validate_agent_bin", { path }),
  /** Set (or clear, with a null/blank path) a per-agent custom binary path.
   *  Persists the setting and refreshes the backend's resolution registry. */
  setAgentBinOverride: (id: string, path: string | null) =>
    invoke<void>("set_agent_bin_override", { id, path }),
  /** Per-agent supported-model discovery (raw ids + any cheap CLI metadata).
   *  The frontend enriches these against models.dev. */
  discoverSupportedModels: () => invoke<AgentModels[]>("discover_supported_models"),

  // ── Workflows v1 (read-only surface; scheduler slices populate the data) ──
  /** Runs newest-updated first, optionally scoped to one project. */
  wfListRuns: (projectId?: string) => invoke<WfRun[]>("wf_list_runs", { projectId }),
  /** A run plus its attempts and messages; null if the run doesn't exist. */
  wfGetRun: (runId: string) => invoke<WfRunDetail | null>("wf_get_run", { runId }),
  /** A page of a run's journal: events strictly after `afterSeq`, oldest first. */
  wfEvents: (runId: string, afterSeq: number, limit: number) =>
    invoke<WfEvent[]>("wf_events", { runId, afterSeq, limit }),
  /** A run's step agents (live + archived). Run-owned agents are hidden from
   *  `get_workspace`, so the monitor fetches them here to render attempt chats. */
  wfRunAgents: (runId: string) => invoke<AgentRecord[]>("wf_run_agents", { runId }),

  // ── Workflows v1: run control (spec §13; registered by the scheduler, S4) ──
  /** Launch a run from a launch-time `spec` snapshot; returns the new run id.
   *  Pass `definitionId` when launching a stored definition (bumps run_count);
   *  `baseBranch` overrides the branch step 1 forks from. */
  wfLaunch: (
    spec: Spec,
    task: string,
    projectId: string,
    repoPath: string,
    definitionId?: string,
    baseBranch?: string,
    /** Absolute paths of files to attach to the run's first prompt, like a chat
     *  message's attachments. Empty by default. */
    attachments: string[] = [],
    /** Explicit fork-point commit (promote-to-workflow). Wins over `baseBranch`
     *  for the fork point; leave undefined for a normal branch-based launch. */
    baseSha?: string,
  ) =>
    invoke<string>("wf_launch", {
      spec,
      task,
      projectId,
      repoPath,
      definitionId,
      baseBranch,
      baseSha,
      attachments,
    }),
  /** Cancel a run: stops the live attempt's agent and marks the run canceled. */
  wfCancel: (runId: string) => invoke<void>("wf_cancel", { runId }),
  /** Approve a run paused on an approval gate: boundary-commit + advance. */
  wfApprove: (runId: string) => invoke<void>("wf_approve", { runId }),
  /** Reject a run paused on an approval gate (spec §9): re-prompt the gated step
   *  with `note` for one more attempt within budget, else pause `blocked_gate`. */
  wfReject: (runId: string, note: string) => invoke<void>("wf_reject", { runId, note }),
  /** The unified diff of `fromSha..toSha` in a run's own repo — used by the review
   *  surface to diff a ferried step ref against the run base. `path` scopes to one
   *  file; omit for the whole diff. */
  wfRunDiff: (runId: string, fromSha: string, toSha: string, path?: string) =>
    invoke<string>("wf_run_diff", { runId, fromSha, toSha, path: path ?? null }),
  /** Retry a run paused on `blocked_gate` / `stalled` with a fresh attempt. */
  wfRetry: (runId: string) => invoke<void>("wf_retry", { runId }),
  /** Resume a paused run (§13). An optional `budgetPatch` additively raises the
   *  run-level caps (turns / tokens / wall_clock_mins) before re-driving — used
   *  to resume a run paused on `budget_exceeded` (§11.2). */
  wfResume: (runId: string, budgetPatch?: Budgets) =>
    invoke<void>("wf_resume", { runId, budgetPatch }),
  /** Resolve a run paused on a merge conflict (§12.3). `mode` is `"agent"`
   *  (spawn a conflict-resolution step) or `"human"` (the user resolved in the
   *  run repo's integration worktree and committed). */
  wfResolveConflict: (runId: string, mode: "agent" | "human") =>
    invoke<void>("wf_resolve_conflict", { runId, mode }),
  /** Answer a run paused on a human question (§10.4): delivers the reply to the
   *  asking step and resumes. `messageId` is the pending `ask` message id. */
  wfAnswer: (projectId: string, runId: string, messageId: string, body: string) =>
    invoke<void>("wf_answer", { projectId, runId, messageId, body }),
  /** Delete a terminal run and everything it owns (§13): its run-owned step
   *  agents (and their chats), `~/.fletch/runs/<id>/`, and its rows. Cascades
   *  over composed sub-runs; rejected while any run in the tree is active. */
  wfDeleteRun: (runId: string) => invoke<void>("wf_delete_run", { runId }),

  // ── Workflows v1: definition storage (spec §13, `wf_def_*`) ──
  /** Validate and persist a workflow definition. Omit `id` to create; pass an
   *  existing id to edit in place (run_count/created_at are preserved). Rejects
   *  with the joined §5.2 validation errors if the spec is invalid. */
  wfDefSave: (spec: Spec, id?: string, hue?: number) =>
    invoke<Definition>("wf_def_save", { spec, id, hue }),
  /** Every stored definition, newest-edited first. */
  wfDefList: () => invoke<Definition[]>("wf_def_list"),
  /** Delete a definition; in-flight runs keep their own launch snapshot. */
  wfDefDelete: (id: string) => invoke<void>("wf_def_delete", { id }),
  /** Serialize a definition to portable YAML (custom-agent specs embedded). */
  wfDefExportYaml: (id: string) => invoke<string>("wf_def_export_yaml", { id }),
  /** Parse + validate a YAML file and resolve it against the local library.
   *  Missing skills / unknown providers come back as warnings, not errors. */
  wfDefImportYaml: (yamlText: string) => invoke<ImportReport>("wf_def_import_yaml", { yamlText }),
};

// ───────────────────────────── Workflows v1 types ───────────────────────────
// Mirror the serialized Rust rows in src-tauri/src/workflow/types.rs.
// JSON-typed columns arrive as parsed objects (`unknown`), not strings.

export type WfRunStatus = "pending" | "running" | "paused" | "done" | "failed" | "canceled";

export type WfPausedReason =
  | "approval"
  | "question"
  | "blocked_gate"
  | "budget_exceeded"
  | "conflict"
  | "stalled";

export type WfAttemptStatus =
  | "pending"
  | "spawning"
  | "running"
  | "gating"
  | "done"
  | "blocked"
  | "awaiting_approval"
  | "error"
  | "abandoned";

export type WfMessageKind = "report" | "ask" | "answer" | "notify" | "decision";

export type WfMessageStatus = "queued" | "delivered" | "answered" | "expired";

export interface WfRun {
  id: string;
  definition_id: string | null;
  parent_run_id: string | null;
  name: string;
  spec: unknown;
  task: string;
  project_id: string;
  repo_path: string;
  run_dir: string;
  branch: string;
  base_sha: string;
  status: WfRunStatus;
  paused_reason: WfPausedReason | null;
  cursor: unknown | null;
  budgets: unknown;
  spent: unknown;
  error: string | null;
  created_at: number;
  updated_at: number;
}

export interface WfStepExec {
  id: string;
  run_id: string;
  step_id: string;
  attempt: number;
  iteration: number;
  agent_id: string | null;
  status: WfAttemptStatus;
  gate_mode: string;
  head_start: string | null;
  head_end: string | null;
  verdict: unknown | null;
  error: string | null;
  started_at: number | null;
  ended_at: number | null;
}

export interface WfEvent {
  run_id: string;
  seq: number;
  ts: number;
  step_exec_id: string | null;
  type: string;
  payload: unknown;
}

export interface WfMessage {
  id: string;
  run_id: string;
  from_step_exec_id: string | null;
  to_step_exec_id: string | null;
  kind: WfMessageKind;
  body: unknown;
  status: WfMessageStatus;
  created_at: number;
  delivered_at: number | null;
}

export interface WfRunDetail {
  run: WfRun;
  attempts: WfStepExec[];
  messages: WfMessage[];
}

/** `wf:event` envelope (§7.2): the addressing fields only — fetch the payload
 *  on demand via `api.wfEvents`. */
export interface WfEventEnvelope {
  run_id: string;
  seq: number;
  type: string;
  ts: number;
  step_exec_id: string | null;
}

/** Fires on every journal append for any run. */
export function onWfEvent(cb: (e: WfEventEnvelope) => void): Promise<UnlistenFn> {
  return listen<WfEventEnvelope>("wf:event", (event) => cb(event.payload));
}

/** Fires whenever a run row changes; carries the full row. */
export function onWfRun(cb: (e: WfRun) => void): Promise<UnlistenFn> {
  return listen<WfRun>("wf:run", (event) => cb(event.payload));
}

/** `wf:run-deleted` fires the deleted run's id after `wf_delete_run` removes its
 *  rows, so the sidebar drops the row instead of upserting it. */
export function onWfRunDeleted(cb: (runId: string) => void): Promise<UnlistenFn> {
  return listen<string>("wf:run-deleted", (event) => cb(event.payload));
}

/** Payload of the `agent-install:state` event: progress of a one-click agent
 *  CLI install (`api.installAgent`). `line` carries installer output while
 *  running; `error` is set on the final `failed` payload. */
export interface AgentInstallEvent {
  id: string;
  phase: "running" | "done" | "failed";
  line?: string;
  error?: string;
}

export function onAgentInstallState(cb: (e: AgentInstallEvent) => void): Promise<UnlistenFn> {
  return listen<AgentInstallEvent>("agent-install:state", (event) => cb(event.payload));
}

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

/** Degraded transcript-ingest status: the vendor CLI's home dir is gone
 *  (`no_root`), its files no longer parse (`format_drift`), or matched files
 *  couldn't be read at all (`read_error`) or only partially (`partial_read`,
 *  records ingested but the tail may be missing). `healthy` is only ever sent
 *  to clear a prior degraded status. */
export type SyncHealthStatus =
  | "healthy"
  | "no_root"
  | "format_drift"
  | "read_error"
  | "partial_read";

export interface SessionSyncHealthEvent {
  agent_id: string;
  provider: string;
  status: SyncHealthStatus;
  /** Current CLI version (for display/logging only), or null if unprobed. */
  version: string | null;
}

/** Fires when an agent's turn-end transcript ingest changes health — drift
 *  detected, or a prior drift cleared. Emitted on change only. */
export function onSessionSyncHealth(cb: (e: SessionSyncHealthEvent) => void): Promise<UnlistenFn> {
  return listen<SessionSyncHealthEvent>("session:sync-health", (event) => cb(event.payload));
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

export function onRunPort(cb: (e: RunPortEvent) => void): Promise<UnlistenFn> {
  return listen<RunPortEvent>("run:port", (event) => cb(event.payload));
}

/** Fires per line (and at start/finish/failure) while the embedded docker agent
 *  image builds on a cold first spawn — feeds the build progress toast. */
export function onDockerBuildProgress(cb: (e: DockerBuildEvent) => void): Promise<UnlistenFn> {
  return listen<DockerBuildEvent>("docker:build-progress", (event) => cb(event.payload));
}
