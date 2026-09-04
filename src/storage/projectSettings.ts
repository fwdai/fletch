import { dbDelete, dbSelect, dbUpsert } from "./db";

export interface ProjectSettingRow {
  project_id: string;
  key: string;
  value: string;
}

export async function getProjectSettings(projectId: string): Promise<Record<string, string>> {
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
  await dbUpsert("project_settings", { project_id: projectId, key, value }, "project_id,key");
}

export async function deleteProjectSetting(projectId: string, key: string): Promise<void> {
  await dbDelete("project_settings", { project_id: projectId, key });
}

/** Per-project switch for autopilot (set in Project Settings). Autopilot is ON
 *  by default for every project, so the row exists only when it has been turned
 *  off: `"0"` = off, absent = on. Turning it back on deletes the row. */
export const AUTOPILOT_ENABLED_KEY = "autopilot.enabled";

/** Whether a stored `autopilot.enabled` value means off. Anything other than an
 *  explicit off reads as on — a loop that is on by default must not switch off
 *  on a typo. */
export function autopilotDisabledValue(value: string | undefined): boolean {
  return value === "0" || value === "false";
}

/** Ids of every project whose autopilot has been switched off, in one query
 *  over the settings table (there is one row per project, keyed by project id). */
export async function loadAutopilotDisabledProjects(): Promise<string[]> {
  const rows = await dbSelect<ProjectSettingRow>("project_settings", {
    where: { key: AUTOPILOT_ENABLED_KEY },
  });
  return rows.filter((r) => autopilotDisabledValue(r.value)).map((r) => r.project_id);
}

/** Per-project keys for the Linear integration (set in Project Settings).
 *  The id scopes which team's issues feed the inbox + composer picker; the
 *  name is display-only so the picker renders without a network round-trip. */
export const LINEAR_TEAM_ID_KEY = "linear.team_id";
export const LINEAR_TEAM_NAME_KEY = "linear.team_name";

/** The project's configured Linear team id, or undefined (also for a blank
 *  `projectId`, e.g. before a draft's project resolves). */
export async function getLinearTeamId(projectId: string): Promise<string | undefined> {
  if (!projectId) return undefined;
  const all = await getProjectSettings(projectId);
  return all[LINEAR_TEAM_ID_KEY] || undefined;
}
