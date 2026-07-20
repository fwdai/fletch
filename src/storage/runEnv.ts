import { getProjectSettings, setProjectSetting } from "./projectSettings";

/** Where a shared variable's value comes from. Mirrors Rust `run_env::Source`
 *  (serialized as a bare lowercase string). */
export type EnvSource = "mirror" | "override";

/** One variable's sharing policy. The value never lives here — mirror values
 *  are read live from `.env`; override values live in the keychain. */
export interface EnvVarCfg {
  key: string;
  shared: boolean;
  source: EnvSource;
}

/** The per-project run-environment document, stored as JSON in
 *  `project_settings` under `run_env`. Mirrors Rust `run_env::RunEnvDoc`. */
export interface RunEnvDoc {
  version: number;
  vars: EnvVarCfg[];
}

const RUN_ENV_KEY = "run_env";
const CURRENT_VERSION = 1;

function emptyDoc(): RunEnvDoc {
  return { version: CURRENT_VERSION, vars: [] };
}

/** Load the project's run-environment document. Absent or malformed → empty
 *  (share nothing), matching the backend's degrade-gracefully posture. */
export async function loadRunEnvDoc(projectId: string): Promise<RunEnvDoc> {
  const settings = await getProjectSettings(projectId);
  const raw = settings[RUN_ENV_KEY];
  if (!raw) return emptyDoc();
  try {
    const parsed = JSON.parse(raw) as Partial<RunEnvDoc>;
    return {
      version: parsed.version ?? CURRENT_VERSION,
      vars: Array.isArray(parsed.vars) ? parsed.vars : [],
    };
  } catch {
    return emptyDoc();
  }
}

/** Persist the run-environment document for a project. */
export async function saveRunEnvDoc(projectId: string, doc: RunEnvDoc): Promise<void> {
  await setProjectSetting(projectId, RUN_ENV_KEY, JSON.stringify(doc));
}

/** Return the config for `key`, or a default (unshared, mirror) if absent. */
export function varConfig(doc: RunEnvDoc, key: string): EnvVarCfg {
  return doc.vars.find((v) => v.key === key) ?? { key, shared: false, source: "mirror" };
}

/** Upsert one variable's config into the document (immutably), dropping a var
 *  that has reverted to the default (unshared + mirror) so the doc stays sparse. */
export function withVar(doc: RunEnvDoc, next: EnvVarCfg): RunEnvDoc {
  const rest = doc.vars.filter((v) => v.key !== next.key);
  const isDefault = !next.shared && next.source === "mirror";
  return { ...doc, vars: isDefault ? rest : [...rest, next] };
}

/** Remove one variable's config from the document (immutably). Used to delete a
 *  variable that isn't backed by `.env` (a user-added or now-stale override) —
 *  `withVar` only drops a var that reverts to the default shape, so removing a
 *  shared/override var needs this. */
export function withoutVar(doc: RunEnvDoc, key: string): RunEnvDoc {
  return { ...doc, vars: doc.vars.filter((v) => v.key !== key) };
}
