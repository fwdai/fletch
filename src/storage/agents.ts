import { dbSelect, dbSelectOne, dbInsert, dbUpdate, dbDelete, dbCount } from "./db";

export interface AgentRow {
  id: string;
  project_id: string;
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

export async function listAgents(projectId?: string): Promise<AgentRow[]> {
  const where: Record<string, unknown> = {};
  if (projectId) where.project_id = projectId;
  return dbSelect<AgentRow>("agents", {
    where,
    orderBy: "created_at",
    orderDirection: "desc",
  });
}

export async function listLiveAgents(projectId?: string): Promise<AgentRow[]> {
  const all = await listAgents(projectId);
  return all.filter((a) => a.archived_at == null);
}

export async function listArchivedAgents(): Promise<AgentRow[]> {
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
