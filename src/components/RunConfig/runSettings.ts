import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "@/storage/projectSettings";

// Run-config overrides live in `project_settings` under a `run.` prefix so
// the table can hold other panels' per-project data without colliding.
const RUN_KEY_PREFIX = "run.";
const runKey = (id: string) => `${RUN_KEY_PREFIX}${id}`;

/** Load the persisted run-config overrides for a project, stripped of the
 *  `run.` prefix so keys match detected-row ids. */
export async function loadRunOverrides(projectId: string): Promise<Record<string, string>> {
  const all = await getProjectSettings(projectId);
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(all)) {
    if (k.startsWith(RUN_KEY_PREFIX)) out[k.slice(RUN_KEY_PREFIX.length)] = v;
  }
  return out;
}

/** Persist reconciled run-config overrides: upsert the set, delete the rest.
 *  Failures are logged, not thrown, so one bad key can't abort the batch. */
export function persistRunOverrides(
  projectId: string,
  toSet: Array<{ id: string; value: string }>,
  toDelete: string[],
): void {
  for (const { id, value } of toSet) {
    setProjectSetting(projectId, runKey(id), value).catch((err) =>
      console.error("setProjectSetting failed", err),
    );
  }
  for (const id of toDelete) {
    deleteProjectSetting(projectId, runKey(id)).catch((err) =>
      console.error("deleteProjectSetting failed", err),
    );
  }
}
