import { invoke } from "../invoke";
import type { Workspace } from "../types/agent";
import type { IssueSummary, PrChecks, PrComments, PrState, PrSummary } from "../types/pr";
import type { GhRepoSummary, GhStatus } from "../types/providers";

export const githubApi = {
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
  getPrState: (agentId: string, subdir?: string) =>
    invoke<PrState | null>("get_pr_state", { agentId, subdir }),
  refreshAllPrStates: () => invoke<Record<string, PrState | null>>("refresh_all_pr_states"),
  refreshAllPrChecks: () => invoke<Record<string, PrChecks | null>>("refresh_all_pr_checks"),
  getPrChecks: (agentId: string, subdir?: string) =>
    invoke<PrChecks | null>("get_pr_checks", { agentId, subdir }),
  getPrComments: (agentId: string, subdir?: string) =>
    invoke<PrComments | null>("get_pr_comments", { agentId, subdir }),
  createPr: (agentId: string, title: string, body: string, subdir?: string) =>
    invoke<PrState>("create_pr", { agentId, title, body, subdir }),
  mergePr: (agentId: string, subdir?: string) => invoke<void>("merge_pr", { agentId, subdir }),
  listPrs: (agentId: string) => invoke<PrSummary[]>("list_prs", { agentId }),
  listRepoPrs: (repoPath: string) => invoke<PrSummary[]>("list_repo_prs", { repoPath }),
  /** Open GitHub issues for the Home inbox, by repo path. `null` when the repo
   *  has no token / non-GitHub origin / a rate-limit pause is active — the
   *  section degrades quietly. `[]` means connected but no open issues. */
  listRepoIssues: (repoPath: string) =>
    invoke<IssueSummary[] | null>("list_repo_issues", { repoPath }),
};
