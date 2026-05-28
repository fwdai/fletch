import { dbSelect, dbSelectOne, dbInsert, dbUpdate, dbDelete, dbCount } from "./db";

export interface AgentRow {
  id: string;
  name: string;
  provider: string;
  task: string;
  status: string;
  view: string;
  session_id: string | null;
  created_at: number;
  last_error: string | null;
  archived_at: number | null;
}

export interface AgentRepoRow {
  id: string;
  agent_id: string;
  repo_path: string;
  subdir: string;
  branch: string | null;
  parent_branch: string | null;
  is_primary: number;
  branch_tip_sha: string | null;
  parent_branch_sha: string | null;
  diff_additions: number;
  diff_deletions: number;
}

export async function listAgents(status?: string): Promise<AgentRow[]> {
  const where: Record<string, unknown> = {};
  if (status) where.status = status;
  return dbSelect<AgentRow>("agents", {
    where,
    orderBy: "created_at",
    orderDirection: "desc",
  });
}

export async function listLiveAgents(): Promise<AgentRow[]> {
  return dbSelect<AgentRow>("agents", {
    where: { archived_at: null },
    orderBy: "created_at",
    orderDirection: "desc",
  });
}

export async function listArchivedAgents(): Promise<AgentRow[]> {
  // archived_at IS NOT NULL — can't express with generic CRUD, use non-null filter in TS
  const all = await dbSelect<AgentRow>("agents", {
    orderBy: "created_at",
    orderDirection: "desc",
  });
  return all.filter((a) => a.archived_at != null);
}

export async function getAgent(id: string): Promise<AgentRow | null> {
  return dbSelectOne<AgentRow>("agents", { where: { id } });
}

export async function insertAgent(
  data: Omit<AgentRow, "created_at">,
): Promise<string> {
  return dbInsert("agents", data as Record<string, unknown>);
}

export async function updateAgent(
  id: string,
  data: Partial<AgentRow>,
): Promise<void> {
  await dbUpdate("agents", { id }, data as Record<string, unknown>);
}

export async function deleteAgent(id: string): Promise<void> {
  await dbDelete("agents", { id });
}

export async function countAgents(
  where?: Record<string, unknown>,
): Promise<number> {
  return dbCount("agents", where);
}

// ── Agent repos ─────────────────────────────────────────────────────────────

export async function listAgentRepos(agentId: string): Promise<AgentRepoRow[]> {
  return dbSelect<AgentRepoRow>("agent_repos", {
    where: { agent_id: agentId },
  });
}

export async function insertAgentRepo(
  data: Omit<AgentRepoRow, "id" | "branch_tip_sha" | "parent_branch_sha" | "diff_additions" | "diff_deletions">,
): Promise<string> {
  return dbInsert("agent_repos", data as Record<string, unknown>);
}

export async function updateAgentRepo(
  id: string,
  data: Partial<AgentRepoRow>,
): Promise<void> {
  await dbUpdate("agent_repos", { id }, data as Record<string, unknown>);
}

export async function deleteAgentRepos(agentId: string): Promise<void> {
  await dbDelete("agent_repos", { agent_id: agentId });
}
