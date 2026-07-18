import { invoke } from "../invoke";
import type { ProjectDeleteResult, Workspace } from "../types/agent";

export const workspaceApi = {
  getWorkspace: () => invoke<Workspace | null>("get_workspace"),
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
};
