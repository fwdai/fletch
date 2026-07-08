import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "@/storage/projectSettings";

// Run-config values live in `project_settings` under a `run.` prefix so the
// table can hold other panels' per-project data without colliding. Two
// scopes share the table:
//   run.<rowId>                 — the project setting (Project Settings page)
//   run.agent.<agentId>.<rowId> — a per-agent override layered on top of it
//                                 (Run panel sheet)
// The backend resolves the same two keys (agent first) in `read_run_commands`.
const RUN_KEY_PREFIX = "run.";
const AGENT_SCOPE_PREFIX = "run.agent.";
const runKey = (id: string, agentId?: string) =>
  agentId ? `${AGENT_SCOPE_PREFIX}${agentId}.${id}` : `${RUN_KEY_PREFIX}${id}`;

/** Load the persisted run-config values for a project — or, when `agentId`
 *  is given, the per-agent overrides — stripped of their prefix so keys
 *  match detected-row ids. */
export async function loadRunOverrides(
  projectId: string,
  agentId?: string,
): Promise<Record<string, string>> {
  const all = await getProjectSettings(projectId);
  const prefix = agentId ? `${AGENT_SCOPE_PREFIX}${agentId}.` : RUN_KEY_PREFIX;
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(all)) {
    if (!k.startsWith(prefix)) continue;
    // Project scope must not slurp up agent-scoped keys.
    if (!agentId && k.startsWith(AGENT_SCOPE_PREFIX)) continue;
    out[k.slice(prefix.length)] = v;
  }
  return out;
}

/** Persist reconciled run-config values: upsert the set, delete the rest.
 *  Pass `agentId` to write the per-agent scope instead of the project's.
 *  Failures are logged, not thrown, so one bad key can't abort the batch. */
export function persistRunOverrides(
  projectId: string,
  toSet: Array<{ id: string; value: string }>,
  toDelete: string[],
  agentId?: string,
): void {
  for (const { id, value } of toSet) {
    setProjectSetting(projectId, runKey(id, agentId), value).catch((err) =>
      console.error("setProjectSetting failed", err),
    );
  }
  for (const id of toDelete) {
    deleteProjectSetting(projectId, runKey(id, agentId)).catch((err) =>
      console.error("deleteProjectSetting failed", err),
    );
  }
}
