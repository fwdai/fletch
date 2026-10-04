import { api } from "@/api";

// Per-project settings are the HOST's: run commands, env sharing, verify,
// roadmap autonomy, autopilot's switch, the Linear team, … (`hostOwnedKeys.ts`)
// live in the host's `project_settings` and are read and written only through
// `api.getProjectSettings` / `api.setProjectSetting` (or `useProjectSettings`)
// and autopilot's own `autopilot_set`, so a project page edits the engine the
// UI is driving (docs/remote-protocol.md, "Settings"). Nothing writes the
// table through the generic bridge any more (`hostOwnedKeys.test.ts` fails the
// build if something starts to).

/** Per-project keys for the Linear integration (set in Project Settings).
 *  The id scopes which team's issues feed the inbox + composer picker; the
 *  name is display-only so the picker renders without a network round-trip.
 *  Host-owned: the host's issue listing is handed the id. */
export const LINEAR_TEAM_ID_KEY = "linear.team_id";
export const LINEAR_TEAM_NAME_KEY = "linear.team_name";

/** The project's configured Linear team id, or undefined (also for a blank
 *  `projectId`, e.g. before a draft's project resolves). */
export async function getLinearTeamId(projectId: string): Promise<string | undefined> {
  if (!projectId) return undefined;
  const all = await api.getProjectSettings(projectId);
  return all[LINEAR_TEAM_ID_KEY] || undefined;
}
