// One facade over the remote client, named and typed like the desktop's
// `src/api/domains/*` so a call site reads the same on both. Every method is a
// thin `call(op, args)` with the desktop's own argument keys — no arg is
// renamed or reshaped anywhere in mobile.

import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type {
  CheckoutFile,
  CheckoutFileContents,
  DiffBaseMode,
  DirListing,
} from "@desktop/api/types/checkout";
import type { GitState, ShortStats } from "@desktop/api/types/git";
import type { PrChecks, PrLive, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import type {
  ItemStatus,
  RoadmapItem,
  RoadmapItemPatch,
  RoadmapItemUpdate,
} from "@desktop/api/types/roadmap";
import type { PublishApproval } from "@desktop/api/types/sandbox";
import type { LiveTurn, SessionRecord, UserTurn } from "@desktop/api/types/session";
import type { AgentModels } from "@desktop/data/modelCatalog/types";
import type { CustomAgent } from "@desktop/storage/customAgents";
import type { HostProvider, RemoteClient } from "../remote";

/** The extras a preset-backed spawn carries. All optional: a plain spawn sends
 *  null for each, exactly as before. */
export interface SpawnOptions {
  /** The preset's brief, sent only as the *fallback*: the host uses it when
   *  `customAgentId` is absent or names a row that is no longer there. A
   *  library with a Project Manager row wins, and its own brief is what runs. */
  instructions?: string | null;
  /** The library row the session runs as. The host resolves it by value — the
   *  row's brief, skills and MCP servers all come off the Mac — so the phone
   *  names the preset and never carries its configuration. */
  customAgentId?: string | null;
  /** What the workspace is for — `ROADMAP_PM_PURPOSE` for a planning chat.
   *  A tagged workspace is hidden from the snapshot and listed on its own. */
  purpose?: string | null;
}

/** A `custom_agents` row as the host stores it: the desktop's `CustomAgent`
 *  with its two id arrays still in their JSON TEXT columns. Typed off the
 *  desktop's own interface so a field added there is not re-declared here. */
export type CustomAgentRow = Omit<CustomAgent, "skillIds" | "mcpServerIds"> & {
  skill_ids: string;
  mcp_server_ids: string;
};

/** The result of discarding a ghost — the delete's answer to
 *  `RoadmapItemUpdate`. `applied` is true only when the row was still
 *  `proposed` and so was deleted, and `item` then has nothing to report. A row
 *  that had already been ruled on comes back untouched as `item`, and one that
 *  is gone already comes back as neither. */
export interface RoadmapItemDiscard {
  applied: boolean;
  item: RoadmapItem | null;
}

export function createApi(client: RemoteClient) {
  const call = <T>(op: string, args?: Record<string, unknown>) => client.call<T>(op, args);
  return {
    getWorkspace: () => call<Workspace | null>("get_workspace"),
    allocateDraftName: (drafts: string[]) => call<string>("allocate_draft_name", { drafts }),
    /** Host forces `view: "custom"`. `skills`/`mcpServers` go null because the
     *  phone never chooses them: it names the preset in `customAgentId` and the
     *  host resolves that row's brief, skills and MCP servers by value, so MCP
     *  configuration never crosses the wire. `options` carries the rest of what
     *  a preset-backed spawn adds — the fallback brief for a library with no
     *  such row, and the purpose tag that keeps a chat off the sidebar. A host
     *  from before those were honoured simply ignores them. */
    spawnAgent: (
      repoPath: string,
      provider: string,
      name: string,
      effort: string | null,
      model: string | null,
      forkBase: string,
      options: SpawnOptions = {},
    ) =>
      call<AgentRecord>("spawn_agent", {
        view: "custom",
        repoPath,
        provider,
        name,
        effort,
        model,
        instructions: options.instructions ?? null,
        customAgentId: options.customAgentId ?? null,
        skills: null,
        mcpServers: null,
        forkBase,
        issueRef: null,
        purpose: options.purpose ?? null,
      }),
    /** A project's purpose-tagged chats, newest first. They are absent from the
     *  `get_workspace` snapshot by design (see `AgentRecord.purpose`), so this
     *  is the only way the phone learns about them. */
    listProjectChats: (projectId: string, purpose: string) =>
      call<AgentRecord[]>("list_project_chats", { projectId, purpose }),
    /** One record by id, purpose-tagged chats included — the read that resolves
     *  an agent nothing has listed yet, which is all a phone launched by a
     *  notification tap has. Gated on `hostSupports("get_agent")`. */
    getAgent: (agentId: string) => call<AgentRecord | null>("get_agent", { agentId }),
    /** The Mac's custom-agent library, as raw rows. The phone only reads the
     *  spawn profile off them (base/model/effort/instructions); the two JSON id
     *  columns are passed over — the host resolves skills and MCP servers. */
    listCustomAgents: () => call<CustomAgentRow[]>("list_custom_agents"),
    sendUserMessage: (agentId: string, turnId: string, text: string, attachments: string[] = []) =>
      call<boolean>("send_user_message", { agentId, turnId, text, attachments }),
    answerToolUse: (
      agentId: string,
      requestId: string,
      updatedInput: unknown,
      behavior: "allow" | "deny" = "allow",
      message?: string,
    ) =>
      call<null>("answer_tool_use", {
        agentId,
        requestId,
        updatedInput,
        behavior,
        message: message ?? null,
      }),
    /** Answer a held `publish:approval-requested` prompt. Gated on
     *  `hostSupports("answer_publish_approval")`: a host from before the op
     *  answers `unknown op`. */
    answerPublishApproval: (id: string, approved: boolean) =>
      call<null>("answer_publish_approval", { id, approved }),
    /** The prompts still waiting, oldest first — what a phone that connected
     *  after the event fired has no other way to learn. Gated on
     *  `hostSupports("approvals_list")`. */
    listPublishApprovals: () => call<PublishApproval[]>("approvals_list"),
    stopAgent: (agentId: string) => call<null>("stop_agent", { agentId }),
    resumeAgent: (agentId: string) => call<null>("resume_agent", { agentId }),
    archiveAgent: (agentId: string) => call<null>("archive_agent", { agentId }),
    /** The destructive twin of `archiveAgent` — record, checkout and transcript
     *  all go. What a planning chat is deleted with: an archive would only hide
     *  it, and nothing on the phone lists an archived chat again. Gated on
     *  `hostSupports("discard_agent")`. */
    discardAgent: (agentId: string) => call<null>("discard_agent", { agentId }),
    setAgentModel: (agentId: string, model: string | null) =>
      call<null>("set_agent_model", { agentId, model }),
    setAgentEffort: (agentId: string, effort: string | null) =>
      call<null>("set_agent_effort", { agentId, effort }),
    readSessionRecords: (agentId: string) =>
      call<SessionRecord[]>("read_session_records", { agentId }),
    readUserTurns: (agentId: string) => call<UserTurn[]>("read_user_turns", { agentId }),
    syncSession: (agentId: string) => call<null>("sync_session", { agentId }),
    /** The running turn's events, which the records lack until it ends. Gated
     *  on `hostSupports("read_live_turn")`. */
    readLiveTurn: (agentId: string) => call<LiveTurn>("read_live_turn", { agentId }),
    getGitState: (agentId: string, subdir?: string) =>
      call<GitState | null>("get_git_state", { agentId, subdir }),
    /** Uncommitted working-tree stats for the whole fleet, keyed by agent id —
     *  the same fleet-wide poll the desktop sidebar reads. */
    getAllShortstats: () => call<Record<string, ShortStats>>("get_all_shortstats"),
    listCheckoutTree: (agentId: string) => call<CheckoutFile[]>("list_checkout_tree", { agentId }),
    readCheckoutFile: (agentId: string, path: string, baseMode?: DiffBaseMode) =>
      call<CheckoutFileContents>("read_checkout_file", { agentId, path, baseMode }),
    getFileDiff: (agentId: string, path: string, baseMode?: DiffBaseMode) =>
      call<string>("get_file_diff", { agentId, path, baseMode }),
    commitAgent: (agentId: string, message: string, subdir?: string) =>
      call<null>("commit_agent", { agentId, message, subdir }),
    pushAgent: (agentId: string, subdir?: string) =>
      call<string>("push_agent", { agentId, subdir }),
    createPr: (agentId: string, title: string, body: string, subdir?: string) =>
      call<PrState>("create_pr", { agentId, title, body, subdir }),
    getPrState: (agentId: string, subdir?: string) =>
      call<PrState | null>("get_pr_state", { agentId, subdir }),
    getPrChecks: (agentId: string, subdir?: string) =>
      call<PrChecks | null>("get_pr_checks", { agentId, subdir }),
    getPrLive: (agentId: string, subdir?: string) =>
      call<PrLive | null>("get_pr_live", { agentId, subdir }),
    listRepoBranches: (repoPath: string) => call<string[]>("list_repo_branches", { repoPath }),
    repoDefaultBranch: (repoPath: string) => call<string>("repo_default_branch", { repoPath }),
    discoverSupportedModels: () => call<AgentModels[]>("discover_supported_models"),
    /** Which provider CLIs the host has, and which of them are signed in — the
     *  agent runs there, so this and not the static `PROVIDERS` list decides
     *  what can be spawned. Gated on `hostSupports("host_providers")`; a host
     *  without it says nothing and the picker offers everything, as before. */
    hostProviders: () => call<HostProvider[]>("host_providers"),
    /** `path` may be `~`-relative; the host expands it and reports the
     *  absolute `base` it read, which is what the picker navigates from. */
    listDir: (path: string) => call<DirListing>("list_dir", { path }),
    addWorkspaceRepo: (repoPath: string) => call<Workspace>("add_workspace_repo", { repoPath }),
    cloneRepo: (spec: string, destParent: string) =>
      call<Workspace>("clone_repo", { spec, destParent }),
    ghStatus: () => call<GhStatus>("gh_status"),
    ghRepoList: () => call<GhRepoSummary[]>("gh_repo_list"),

    /** A project's whole roadmap. The phone reads it for one thing — the PM's
     *  `proposed` ghosts, which a planning chat draws as decision cards — so the
     *  filtering is the caller's, not a second op. */
    roadmapListItems: (projectId: string) =>
      call<RoadmapItem[]>("roadmap_list_items", { projectId }),
    /** Patch an item and get the stored row back. `expectStatus` makes it a
     *  *conditional* transition: the patch lands only while the row still says
     *  that status, and a miss comes back as `applied: false` with the row as it
     *  really is. `queue` is the accept-and-dispatch gesture — where the item
     *  actually lands is the host's call (the project's autoqueue dial, a hold),
     *  which is why the answer carries the row rather than an assumed status.
     *
     *  Both are always sent, present-and-null rather than absent, exactly as the
     *  desktop's `roadmapUpdateItem` sends them. */
    roadmapUpdateItem: (
      id: string,
      patch: RoadmapItemPatch,
      expectStatus?: ItemStatus,
      queue?: boolean,
    ) =>
      call<RoadmapItemUpdate>("roadmap_update_item", {
        id,
        patch,
        expectStatus: expectStatus ?? null,
        queue: queue ?? null,
      }),
    /** Say no to a ghost — the other half of ruling on one. A row nobody
     *  accepted was never a roadmap item, so this deletes rather than rejects (a
     *  rejection is for a row that made the board).
     *
     *  Conditional like `roadmapUpdateItem`'s accept, and for the same reason: a
     *  card can sit on screen for minutes, and a discard tapped after someone
     *  else accepted the row must not delete work already under way. */
    roadmapDiscardProposal: (id: string) =>
      call<RoadmapItemDiscard>("roadmap_discard_proposal", { id }),

    /** Remote-only (docs/remote-protocol.md, "Push notifications"): where the
     *  host should have the relay send this phone's alerts. A token needs its
     *  environment — the host rejects one without it — while clearing is
     *  `token: null` on its own. */
    registerPush: (token: string | null, environment?: "sandbox" | "production") =>
      call<null>("register_push", token === null ? { token: null } : { token, environment }),

    /** Remote-only (docs/remote-protocol.md, "Dictation"): the phone captures,
     *  the Mac transcribes with its local whisper engine. `pcm` is base64 of
     *  16-bit little-endian mono samples at `rate` Hz; the transcript is the
     *  reply to `dictationEnd`. */
    dictationStatus: () => call<DictationStatus>("dictation_status"),
    dictationBegin: () => call<DictationBegun>("dictation_begin"),
    dictationAudio: (session: string, rate: number, pcm: string) =>
      call<null>("dictation_audio", { session, rate, pcm }),
    dictationEnd: (session: string) => call<{ text: string }>("dictation_end", { session }),
    dictationCancel: (session: string) => call<null>("dictation_cancel", { session }),

    /** Remote-only (docs/remote-protocol.md, "Attachments"): a file's bytes,
     *  in base64 slices, staged on the Mac where a desktop paste lands. The
     *  `path` from `attachmentEnd` goes in `sendUserMessage`'s attachments. */
    attachmentBegin: (name: string) => call<{ upload: string }>("attachment_begin", { name }),
    attachmentChunk: (upload: string, data: string) =>
      call<null>("attachment_chunk", { upload, data }),
    attachmentEnd: (upload: string) => call<{ path: string }>("attachment_end", { upload }),
    attachmentCancel: (upload: string) => call<null>("attachment_cancel", { upload }),
  };
}

/** Whether the Mac can transcribe for this phone right now. `reason` is shown
 *  as-is when it can't (the local engine is off, or its model isn't downloaded). */
export interface DictationStatus {
  available: boolean;
  reason: string | null;
}

/** A session the Mac has opened for us. */
export interface DictationBegun {
  session: string;
  /** Settings › Dictation's "Stop after a pause", as the Mac had it the moment
   *  this session opened. The pause is heard here — the Mac only ever sees the
   *  chunks this phone chose to send — so honouring the setting is the phone's
   *  job; see `DictationSession.start`.
   *
   *  It arrives per session rather than with the availability probe because the
   *  probe only runs on mount and reconnect: a phone left connected would
   *  answer every session from one stale read. Absent from a host too old to
   *  report it, which is why the reading is `!== false` — auto-stop on is what
   *  those hosts have always done. */
  auto_stop?: boolean;
}

export type Api = ReturnType<typeof createApi>;
