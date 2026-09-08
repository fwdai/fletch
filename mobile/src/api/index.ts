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
import type { DiffStats, GitState } from "@desktop/api/types/git";
import type { PrChecks, PrLive, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import type { SessionRecord, UserTurn } from "@desktop/api/types/session";
import type { AgentModels } from "@desktop/data/modelCatalog/types";
import type { RemoteClient } from "../remote";

export function createApi(client: RemoteClient) {
  const call = <T>(op: string, args?: Record<string, unknown>) => client.call<T>(op, args);
  return {
    getWorkspace: () => call<Workspace | null>("get_workspace"),
    allocateDraftName: (drafts: string[]) => call<string>("allocate_draft_name", { drafts }),
    /** Host forces `view: "custom"` and ignores purpose/skills/mcp in v1, so
     *  only the fields the phone can actually influence are sent. */
    spawnAgent: (
      repoPath: string,
      provider: string,
      name: string,
      effort: string | null,
      model: string | null,
      forkBase: string,
    ) =>
      call<AgentRecord>("spawn_agent", {
        view: "custom",
        repoPath,
        provider,
        name,
        effort,
        model,
        instructions: null,
        customAgentId: null,
        skills: null,
        mcpServers: null,
        forkBase,
        issueRef: null,
        purpose: null,
      }),
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
    stopAgent: (agentId: string) => call<null>("stop_agent", { agentId }),
    resumeAgent: (agentId: string) => call<null>("resume_agent", { agentId }),
    archiveAgent: (agentId: string) => call<null>("archive_agent", { agentId }),
    setAgentModel: (agentId: string, model: string | null) =>
      call<null>("set_agent_model", { agentId, model }),
    setAgentEffort: (agentId: string, effort: string | null) =>
      call<null>("set_agent_effort", { agentId, effort }),
    readSessionRecords: (agentId: string) =>
      call<SessionRecord[]>("read_session_records", { agentId }),
    readUserTurns: (agentId: string) => call<UserTurn[]>("read_user_turns", { agentId }),
    getGitState: (agentId: string, subdir?: string) =>
      call<GitState | null>("get_git_state", { agentId, subdir }),
    getAgentDiffStats: (agentId: string) => call<DiffStats>("get_agent_diff_stats", { agentId }),
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
    /** `path` may be `~`-relative; the host expands it and reports the
     *  absolute `base` it read, which is what the picker navigates from. */
    listDir: (path: string) => call<DirListing>("list_dir", { path }),
    addWorkspaceRepo: (repoPath: string) => call<Workspace>("add_workspace_repo", { repoPath }),
    cloneRepo: (spec: string, destParent: string) =>
      call<Workspace>("clone_repo", { spec, destParent }),
    ghStatus: () => call<GhStatus>("gh_status"),
    ghRepoList: () => call<GhRepoSummary[]>("gh_repo_list"),

    /** Remote-only (docs/remote-protocol.md, "Push notifications"): where the
     *  host should have the relay send this phone's alerts. A token needs its
     *  environment — the host rejects one without it — while clearing is
     *  `token: null` on its own. */
    registerPush: (token: string | null, environment?: "sandbox" | "production") =>
      call<null>("register_push", token === null ? { token: null } : { token, environment }),
  };
}

export type Api = ReturnType<typeof createApi>;
