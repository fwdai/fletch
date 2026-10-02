import type { McpServerSnapshot } from "@/storage/mcpServers";
import type { SkillSnapshot } from "@/storage/skills";
import { invoke } from "../invoke";
import type {
  AgentRecord,
  AgentView,
  ForkCode,
  ForkContext,
  RestoreReport,
  RewindOutcome,
  RewindScope,
  TrackedRepo,
} from "../types/agent";

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
    /** The first prompt, when the caller sends it right after spawning (the
     *  draft path). Persisted as the record's `task` at creation, so the
     *  returned record and every snapshot already carry it — the sidebar row
     *  never shows an empty task while the process starts. */
    task?: string,
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
      task: task ?? null,
    }),
  /** A project's purpose-tagged chats, newest first (see `ROADMAP_PM_PURPOSE`).
   *  These never appear in the workspace snapshot, so the surface that owns
   *  them lists them here; the records are ordinary agents otherwise. */
  listProjectChats: (projectId: string, purpose: string) =>
    invoke<AgentRecord[]>("list_project_chats", { projectId, purpose }),
  /** Fork an existing workspace into a new one at an anchor — through the turn
   *  `turnId` names, or the end of the parent's session when `null` — seeding
   *  its worktree (`code`) and conversation (`context`) independently. A
   *  carried conversation is referenced, not copied: the child's history
   *  starts with the parent's, through the anchor.
   *
   *  `transcript` is the carried range rendered as text (adapters/handoff),
   *  from the normalized chat log so it works uniformly across every provider;
   *  the backend summarizes it to brief the child's fresh agent. `null` when
   *  nothing is carried. */
  forkAgent: (
    parentId: string,
    turnId: string | null,
    code: ForkCode,
    context: ForkContext,
    transcript: string | null,
  ) => invoke<AgentRecord>("fork_agent", { parentId, turnId, code, context, transcript }),
  /** Rewind an agent in place to just before the turn `turnId` names: its
   *  conversation, its code, or both. `transcript` is the conversation before
   *  that turn rendered as text (adapters/handoff), for the summary the
   *  rewound agent is briefed with when it can't resume the conversation
   *  natively. Resolves once the rewound agent is up. */
  rewindAgent: (agentId: string, turnId: string, scope: RewindScope, transcript: string | null) =>
    invoke<RewindOutcome>("rewind_agent", { agentId, turnId, scope, transcript }),
  /** What rewinding the code to before `turnId` would do; rejects with why it
   *  can't when it can't. */
  previewRewindCode: (agentId: string, turnId: string) =>
    invoke<RestoreReport>("preview_rewind_code", { agentId, turnId }),
  /** Undo a rewind's code restore, from the report the rewind returned. */
  undoCodeRestore: (agentId: string, report: RestoreReport) =>
    invoke<void>("undo_code_restore", { agentId, report }),
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
