import { invoke } from "../invoke";
import type {
  AutopilotLogEntry,
  AutopilotSnapshot,
  DelegationEvent,
  GitMeta,
  GitState,
  ShortStats,
} from "../types/git";

export const gitApi = {
  /** The current HEAD commit SHA of an agent's checkout (primary repo). The
   *  fork point for "promote to workflow". */
  agentHeadSha: (agentId: string) => invoke<string>("agent_head_sha", { agentId }),
  // Git/PR commands below take an optional `subdir` (a checkout's directory
  // name from `TrackedRepo.subdir`) to target one repo of a multi-repo agent.
  // Omitted/undefined serializes to None = the agent's primary (first) repo.
  getGitState: (agentId: string, subdir?: string) =>
    invoke<GitState | null>("get_git_state", { agentId, subdir }),
  getAllShortstats: () => invoke<Record<string, ShortStats>>("get_all_shortstats"),
  getAllGitMeta: () => invoke<Record<string, GitMeta>>("get_all_git_meta"),
  /** Hand a git playbook (`action`) to the agent. The host composes the
   *  trigger, holds it while the agent is mid-turn and watches it to its end;
   *  resolves to the delegation as recorded (`queued` or `started`). */
  delegateGit: (
    agentId: string,
    action: string,
    params?: Record<string, string>,
    subdir?: string,
  ) => invoke<DelegationEvent>("delegate_git", { agentId, subdir, action, params }),
  /** Every delegation the host is tracking, to resync the mirror. */
  getDelegations: () => invoke<DelegationEvent[]>("get_delegations"),
  /** Autopilot as the host runs it — every live agent's checkouts and the two
   *  opt-out lists — to resync the mirror. */
  getAutopilotState: () => invoke<AutopilotSnapshot>("autopilot_state", {}),
  /** What autopilot did on every checkout, newest first. */
  getAutopilotLog: () => invoke<AutopilotLogEntry[]>("autopilot_log", {}),
  /** Flip a project's autopilot switch, or pause / resume one agent. Answers
   *  with the whole host's state after the change. */
  setAutopilot: (target: { projectId: string } | { agentId: string }, enabled: boolean) =>
    invoke<AutopilotSnapshot>("autopilot_set", { ...target, enabled }),
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
  /** Unset the config keys `GitState.blocked_config` names. */
  clearCheckoutConfig: (agentId: string, subdir?: string) =>
    invoke<void>("clear_checkout_config", { agentId, subdir }),
  deleteBranchAgent: (agentId: string, subdir?: string) =>
    invoke<void>("delete_branch_agent", { agentId, subdir }),
  listRepoBranches: (repoPath: string) => invoke<string[]>("list_repo_branches", { repoPath }),
  /** The repo's default branch, resolved from its remote — what the new-agent
   *  screen pre-selects as the base. Never the branch the user currently has
   *  checked out, and never a hardcoded "main" (which forks the wrong branch on
   *  a master/develop repo). Infallible backend-side; falls back to "main". */
  repoDefaultBranch: (repoPath: string) => invoke<string>("repo_default_branch", { repoPath }),
};
