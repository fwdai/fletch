import { dbSelect, dbDelete, dbUpsert } from "./db";

export interface ProjectSettingRow {
  project_id: string;
  key: string;
  value: string;
}

export async function getProjectSettings(
  projectId: string,
): Promise<Record<string, string>> {
  const rows = await dbSelect<ProjectSettingRow>("project_settings", {
    where: { project_id: projectId },
  });
  const out: Record<string, string> = {};
  for (const row of rows) {
    out[row.key] = row.value;
  }
  return out;
}

export async function setProjectSetting(
  projectId: string,
  key: string,
  value: string,
): Promise<void> {
  await dbUpsert(
    "project_settings",
    { project_id: projectId, key, value },
    "project_id,key",
  );
}

export async function deleteProjectSetting(
  projectId: string,
  key: string,
): Promise<void> {
  await dbDelete("project_settings", { project_id: projectId, key });
}
