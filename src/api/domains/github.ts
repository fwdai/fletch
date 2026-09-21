import { invoke, invokeLocal } from "../invoke";
import type { Workspace } from "../types/agent";
import type { AgentPrStatus, PrChecks, PrComments, PrLive, PrState, PrSummary } from "../types/pr";
import type { GhRepoSummary, GhStatus } from "../types/providers";

export const githubApi = {
  /** THIS Mac's GitHub login — the identity Settings › Account and onboarding
   *  show and act on, since signing in (`oauth_device_login`) and signing out
   *  are local by construction: a host manages its own with `fletch-host github
   *  login`. */
  ghStatus: () => invokeLocal<GhStatus>("gh_status"),
  /** The ACTIVE environment's GitHub login, for the push / PR / clone gates —
   *  those act where the checkout is, so a host's answer is the one that
   *  decides whether they can work. */
  ghStatusActive: () => invoke<GhStatus>("gh_status"),
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
  /** Drops the token in THIS Mac's store. Never routed: the op is not on a
   *  host's table, and the account it would clear is not the one the Settings
   *  button signed in. */
  githubDisconnect: () => invokeLocal<void>("github_disconnect"),
  getPrState: (agentId: string, subdir?: string) =>
    invoke<PrState | null>("get_pr_state", { agentId, subdir }),
  refreshAllPrStatus: () => invoke<Record<string, AgentPrStatus>>("refresh_all_pr_status"),
  getPrChecks: (agentId: string, subdir?: string) =>
    invoke<PrChecks | null>("get_pr_checks", { agentId, subdir }),
  getPrLive: (agentId: string, subdir?: string) =>
    invoke<PrLive | null>("get_pr_live", { agentId, subdir }),
  getPrHistory: (agentId: string, subdir?: string) =>
    invoke<PrState[]>("get_pr_history", { agentId, subdir }),
  getPrThreads: (agentId: string, subdir?: string) =>
    invoke<PrComments | null>("get_pr_threads", { agentId, subdir }),
  createPr: (agentId: string, title: string, body: string, subdir?: string) =>
    invoke<PrState>("create_pr", { agentId, title, body, subdir }),
  mergePr: (agentId: string, subdir?: string) => invoke<void>("merge_pr", { agentId, subdir }),
  listPrs: (agentId: string) => invoke<PrSummary[]>("list_prs", { agentId }),
  listRepoPrs: (repoPath: string) => invoke<PrSummary[]>("list_repo_prs", { repoPath }),
};
