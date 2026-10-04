// The settings the HOST reads — its alerts, its idle sweep, its sandbox and
// spawn path, its publishing, and each project's run/verify/roadmap rows. All of
// them follow the active environment, so Settings and the project page edit the
// engine the UI is driving, never this Mac's database behind its back
// (docs/remote-protocol.md, "Settings"). Each setter persists the key, updates
// the host's in-memory mirror and emits `settings:changed` /
// `project_settings:changed`; never write one of these keys with `setSetting`
// or `dbUpsert` (src/storage/hostOwnedKeys.test.ts enforces it).
import type { SandboxEngine } from "@/storage/preferences";
import { invoke } from "../invoke";

export const settingsApi = {
  /** The host-owned global settings as stored, and nothing else (no secret,
   *  no client preference). An absent key is unset: apply its default. */
  getSettings: () => invoke<Record<string, string>>("get_settings"),
  /** Whether a finished turn alerts at all (chime, banner, phone push). */
  setNotifyTurnComplete: (enabled: boolean) =>
    invoke<void>("set_notify_turn_complete", { enabled }),
  /** The ship-loop alerts (checks settled, review comment, PR merged/closed). */
  setNotifyPrActivity: (enabled: boolean) => invoke<void>("set_notify_pr_activity", { enabled }),
  /** Days a workspace may sit idle before the host's sweep archives it; 0 = off. */
  setAutoArchiveIdleDays: (days: number) => invoke<void>("set_auto_archive_idle_days", { days }),
  /** Code indexing (codegraph); enabling warms the index in the background. */
  setCodeIndexingEnabled: (enabled: boolean) =>
    invoke<void>("set_code_indexing_enabled", { enabled }),
  /** The engine new agents are stamped with. The host probes a container
   *  engine live and refuses one whose runtime is unreachable. */
  setSandboxEngine: (engine: SandboxEngine) => invoke<void>("set_sandbox_engine", { engine }),
  /** Docker launch knobs, written together; blank/null clears one. */
  setDockerLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_docker_launch_settings", { image, memory, cpus }),
  /** The podman twin, over its own keys. */
  setPodmanLaunchSettings: (image: string | null, memory: string | null, cpus: string | null) =>
    invoke<void>("set_podman_launch_settings", { image, memory, cpus }),
  /** Set (or clear, with a null/blank path) a per-agent custom binary path —
   *  a path on the host. The host respawns that provider's live agents. */
  setAgentBinOverride: (id: string, path: string | null) =>
    invoke<void>("set_agent_bin_override", { id, path }),
  /** Validated and normalized host-side; resolves to the stored form. */
  setBranchPrefix: (prefix: string) => invoke<string>("set_branch_prefix", { prefix }),
  setDraftPrs: (enabled: boolean) => invoke<void>("set_draft_prs", { enabled }),
  /** Whether an agent must get the user's approval before publishing. Off by
   *  default: autopilot publishes unattended. */
  setPublishConfirmation: (enabled: boolean) =>
    invoke<void>("set_publish_confirmation", { enabled }),
  /** Seconds a publish approval waits before denying; 0 = until answered. */
  setPublishApprovalWait: (secs: number) => invoke<void>("set_publish_approval_wait", { secs }),
  /** "Remove agent attribution": true strips agents' commit/PR attribution
   *  whatever their own settings say. */
  setAgentAttributionRemoved: (removed: boolean) =>
    invoke<void>("set_agent_attribution_removed", { removed }),
  /** One project's client-writable settings (the host's allowlist). */
  getProjectSettings: (projectId: string) =>
    invoke<Record<string, string>>("get_project_settings", { projectId }),
  /** Write one project setting; `null` deletes the row (back to the default).
   *  The host refuses a key outside its allowlist. */
  setProjectSetting: (projectId: string, key: string, value: string | null) =>
    invoke<void>("set_project_setting", { projectId, key, value }),
};
