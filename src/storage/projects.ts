import { dbSelect, dbSelectOne, dbInsert, dbUpdate, dbDelete } from "./db";

export interface ProjectRow {
  id: string;
  name: string;
  created_at: number;
}

export interface RepoRow {
  id: string;
  project_id: string;
  path: string;
  created_at: number;
}

export async function listProjects(): Promise<ProjectRow[]> {
  return dbSelect<ProjectRow>("projects", {
    orderBy: "created_at",
    orderDirection: "asc",
  });
}

export async function getProject(id: string): Promise<ProjectRow | null> {
  return dbSelectOne<ProjectRow>("projects", { where: { id } });
}

export async function insertProject(name: string): Promise<string> {
  return dbInsert("projects", { name });
}

export async function updateProject(
  id: string,
  data: Partial<Omit<ProjectRow, "id" | "created_at">>,
): Promise<void> {
  await dbUpdate("projects", { id }, data as Record<string, unknown>);
}

export async function deleteProject(id: string): Promise<void> {
  await dbDelete("projects", { id });
}

// ── Repos ───────────────────────────────────────────────────────────────────

export async function listRepos(projectId: string): Promise<RepoRow[]> {
  return dbSelect<RepoRow>("repos", {
    where: { project_id: projectId },
    orderBy: "created_at",
    orderDirection: "asc",
  });
}

export async function getRepoByPath(path: string): Promise<RepoRow | null> {
  return dbSelectOne<RepoRow>("repos", { where: { path } });
}

export async function insertRepo(
  projectId: string,
  path: string,
): Promise<string> {
  return dbInsert("repos", { project_id: projectId, path });
}

export async function deleteRepo(id: string): Promise<void> {
  await dbDelete("repos", { id });
}
