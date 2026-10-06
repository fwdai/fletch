// The setting keys the HOST reads, and so owns. Mirrors the two allowlists in
// `crates/fletch-core/src/commands/{settings,project_settings}.rs`
// (docs/remote-protocol.md, "Settings"): a client reads them with
// `get_settings` / `get_project_settings` and writes them only through the
// dedicated ops in `api/domains/settings.ts` — never `setSetting`,
// `setProjectSetting` or `dbUpsert`, which write THIS Mac's database whatever
// environment is active. `hostOwnedKeys.test.ts` scans `src/` for that.

/** Global `settings` keys the host reads. */
export const HOST_SETTING_KEYS = [
  "notify_turn_complete",
  "notify_pr_activity",
  "auto_archive_idle_days",
  "code_indexing_enabled",
  "context_layer_enabled",
  "sandbox_engine",
  "docker_image",
  "docker_memory",
  "docker_cpus",
  "podman_image",
  "podman_memory",
  "podman_cpus",
  "git_branch_prefix",
  "github_draft_prs",
  "publish_confirmation",
  "publish_approval_wait",
  "agent_attribution_removed",
] as const;

/** `agent_bin_path_<id>`: one provider's custom binary, a path on the host. */
export const HOST_SETTING_PREFIXES = ["agent_bin_path_"] as const;

export function isHostSettingKey(key: string): boolean {
  return (
    (HOST_SETTING_KEYS as readonly string[]).includes(key) ||
    HOST_SETTING_PREFIXES.some((p) => key.startsWith(p) && key.length > p.length)
  );
}

/** `project_settings` keys the host reads and a client may write. */
export const HOST_PROJECT_SETTING_KEYS = [
  "verify.on_turn_end",
  "run_env",
  "workflow.default",
  "composer.mode",
  "roadmap.autoqueue",
  "roadmap.max_concurrent",
  "roadmap.settle_review",
  "roadmap.midrun_awareness",
  "roadmap.declined_issues",
  "linear.team_id",
  "linear.team_name",
  "context.enabled",
  "context.extract",
] as const;

/** `run.<row>` and `run.agent.<agentId>.<row>`. */
export const HOST_PROJECT_SETTING_PREFIXES = ["run."] as const;

export function isHostProjectSettingKey(key: string): boolean {
  return (
    (HOST_PROJECT_SETTING_KEYS as readonly string[]).includes(key) ||
    HOST_PROJECT_SETTING_PREFIXES.some((p) => key.startsWith(p) && key.length > p.length)
  );
}

/** The host-owned slice of a flat settings map. */
export function pickHostSettings(all: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(all)) {
    if (isHostSettingKey(key)) out[key] = value;
  }
  return out;
}
