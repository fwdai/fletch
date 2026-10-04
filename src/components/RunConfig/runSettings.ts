import { api } from "@/api";

// Run-config values live in the host's `project_settings` under a `run.` prefix
// so the table can hold other panels' per-project data without colliding. Two
// scopes share the table:
//   run.<rowId>                 — the project setting (Project Settings page)
//   run.agent.<agentId>.<rowId> — a per-agent override layered on top of it
//                                 (Run panel sheet)
// The host resolves the same two keys (agent first) in `read_run_commands`, so
// both are read and written through the host's project-settings ops, never this
// Mac's table (docs/remote-protocol.md, "Settings").
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
  const all = await api.getProjectSettings(projectId);
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
  const writes: Array<[string, string | null]> = [
    ...toSet.map(({ id, value }): [string, string | null] => [runKey(id, agentId), value]),
    ...toDelete.map((id): [string, string | null] => [runKey(id, agentId), null]),
  ];
  for (const [key, value] of writes) {
    api
      .setProjectSetting(projectId, key, value)
      .catch((err) => console.error(`save ${key} failed`, err));
  }
}
