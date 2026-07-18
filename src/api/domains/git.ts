import { invoke } from "../invoke";
import type { DiffStats, GitMeta, GitState, ShortStats } from "../types/git";

export const gitApi = {
  getAgentDiffStats: (agentId: string) => invoke<DiffStats>("get_agent_diff_stats", { agentId }),
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
  refreshBaseFreshness: () => invoke<void>("refresh_base_freshness"),
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
};
