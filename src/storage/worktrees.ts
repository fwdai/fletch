import { dbSelect, dbInsert, dbUpdate, dbDelete } from "./db";

export interface WorktreeRow {
  id: string;
  agent_id: string;
  repo_id: string;
  subdir: string;
  branch: string | null;
  parent_branch: string | null;
  branch_tip_sha: string | null;
  parent_branch_sha: string | null;
  diff_additions: number;
  diff_deletions: number;
}

export async function listWorktrees(agentId: string): Promise<WorktreeRow[]> {
  return dbSelect<WorktreeRow>("worktrees", {
    where: { agent_id: agentId },
  });
}

export async function insertWorktree(
  data: Omit<WorktreeRow, "id" | "branch_tip_sha" | "parent_branch_sha" | "diff_additions" | "diff_deletions">,
): Promise<string> {
  return dbInsert("worktrees", data as Record<string, unknown>);
}

export async function updateWorktree(
  id: string,
  data: Partial<WorktreeRow>,
): Promise<void> {
  await dbUpdate("worktrees", { id }, data as Record<string, unknown>);
}

export async function deleteWorktrees(agentId: string): Promise<void> {
  await dbDelete("worktrees", { agent_id: agentId });
}
