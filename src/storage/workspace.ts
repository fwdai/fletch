import { dbSelect, dbInsert, dbDelete } from "./db";

export interface WorkspaceRepoRow {
  id: string;
  repo_path: string;
  created_at: number;
}

export async function listWorkspaceRepos(): Promise<WorkspaceRepoRow[]> {
  return dbSelect<WorkspaceRepoRow>("workspace_repos", {
    orderBy: "created_at",
    orderDirection: "asc",
  });
}

export async function addWorkspaceRepo(repoPath: string): Promise<string> {
  return dbInsert("workspace_repos", { repo_path: repoPath });
}

export async function removeWorkspaceRepo(repoPath: string): Promise<void> {
  await dbDelete("workspace_repos", { repo_path: repoPath });
}
