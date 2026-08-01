import type { McpServerSnapshot } from "@/storage/mcpServers";
import type { SkillSnapshot } from "@/storage/skills";
import { invoke } from "../invoke";
import type { AgentRecord, AgentView, ForkCode, ForkContext, TrackedRepo } from "../types/agent";

export const agentsApi = {
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
    /** Tags a workspace owned by a surface other than the sidebar (see
     *  `ROADMAP_PM_PURPOSE`): it is hidden from the sidebar and spawned with a
     *  narrower capability grant. `undefined` for a normal spawn. */
    purpose?: string,
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
      purpose: purpose ?? null,
    }),
  /** A project's purpose-tagged chats, newest first (see `ROADMAP_PM_PURPOSE`).
   *  These never appear in the workspace snapshot, so the surface that owns
   *  them lists them here; the records are ordinary agents otherwise. */
  listProjectChats: (projectId: string, purpose: string) =>
    invoke<AgentRecord[]>("list_project_chats", { projectId, purpose }),
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
  sendUserMessage: (agentId: string, turnId: string, text: string, attachments: string[] = []) =>
    invoke<boolean>("send_user_message", {
      agentId,
      turnId,
      text,
      attachments,
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
  setAgentEffort: (agentId: string, effort: string | null) =>
    invoke<void>("set_agent_effort", { agentId, effort }),
  setAgentModel: (agentId: string, model: string | null) =>
    invoke<void>("set_agent_model", { agentId, model }),
  resumeAgent: (agentId: string) => invoke<void>("resume_agent", { agentId }),
  stopAgent: (agentId: string) => invoke<void>("stop_agent", { agentId }),
  discardAgent: (agentId: string) => invoke<void>("discard_agent", { agentId }),
  archiveAgent: (agentId: string) => invoke<void>("archive_agent", { agentId }),
  restoreAgent: (agentId: string) => invoke<void>("restore_agent", { agentId }),
  addRepoToAgent: (agentId: string, repoPath: string) =>
    invoke<TrackedRepo>("add_repo_to_agent", { agentId, repoPath }),
  /** Name for a new draft. Pass only the *open drafts'* names — the backend
   *  reads live agents from the DB itself, so callers can't over-reserve. */
  allocateDraftName: (drafts: string[]) => invoke<string>("allocate_draft_name", { drafts }),
};
