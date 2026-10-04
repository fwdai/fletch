// The host-owned global settings, as the store holds them: read from the engine
// the UI is driving, folded into the slices' fields, and kept live by
// `settings:changed`. The local environment reads its own `settings` table (the
// generic bridge it always used); a remote one asks the host with
// `get_settings`, since its values are the host's and live in the host's
// database (docs/remote-protocol.md, "Settings").

import { api, type SettingsChangedEvent } from "@/api";
import { hostSupports } from "@/remote/types";
import { pickHostSettings } from "@/storage/hostOwnedKeys";
import {
  parseAutoArchiveIdleDays,
  parseProviderPathOverrides,
  parsePublishApprovalWait,
  parseSandboxEngine,
} from "@/storage/preferences";
import { getAllSettings } from "@/storage/settings";
import { activeEnvironment, forActiveEnvironment } from "./environments";
import type { AppState } from "./types";

type Patch = (patch: Partial<AppState>) => void;

/** The store fields the host-owned keys decide. Every default here is the one
 *  the host's own parser applies to an absent or unreadable value, so a key the
 *  host has never written reads the same on both sides. */
export function hostSettingsState(s: Record<string, string>): Partial<AppState> {
  return {
    // Opt-out, both alerts: only an explicit "false" silences them.
    notifyTurnComplete: s.notify_turn_complete !== "false",
    notifyPrActivity: s.notify_pr_activity !== "false",
    providerPathOverrides: parseProviderPathOverrides(s),
    // Opt-in: only an explicit "true" removes attribution (`attribution::parse`).
    agentAttributionRemoved: s.agent_attribution_removed === "true",
    // Opt-out: only an explicit "false" disables indexing.
    codeIndexingEnabled: s.code_indexing_enabled !== "false",
    sandboxEngine: parseSandboxEngine(s.sandbox_engine),
    // Opt-in, unlike the alerts: autopilot publishes unattended, so defaulting
    // it on would hang every unattended run until the decision timeout.
    publishConfirmation: s.publish_confirmation === "true",
    publishApprovalWait: parsePublishApprovalWait(s.publish_approval_wait),
    autoArchiveIdleDays: parseAutoArchiveIdleDays(s.auto_archive_idle_days),
    branchPrefix: s.git_branch_prefix || "",
    draftPrs: s.github_draft_prs === "true",
    // Blank = unset (launch defaults apply).
    dockerImage: s.docker_image || "",
    dockerMemory: s.docker_memory || "",
    dockerCpus: s.docker_cpus || "",
    podmanImage: s.podman_image || "",
    podmanMemory: s.podman_memory || "",
    podmanCpus: s.podman_cpus || "",
  };
}

/** What the store last read for the active environment — the base a
 *  `settings:changed` event is folded over, since one key can decide a field
 *  that reads several (`providerPathOverrides`). */
let current: Record<string, string> = {};

export function applyHostSettings(set: Patch, settings: Record<string, string>) {
  current = { ...settings };
  set(hostSettingsState(current));
}

/** Fold one write — this window's, another desktop's, a phone's — over what was
 *  read, without a refetch. */
export function applyHostSettingChange(set: Patch, e: SettingsChangedEvent) {
  const next = { ...current };
  if (e.value === null) delete next[e.key];
  else next[e.key] = e.value;
  applyHostSettings(set, next);
}

/** The active environment's host-owned settings, or `null` when it cannot say:
 *  a host from before `get_settings`, whose Settings controls are gated closed
 *  anyway (`hostSettings` / `publishSettings`), keeps whatever was on screen. */
async function readHostSettings(): Promise<Record<string, string> | null> {
  const env = activeEnvironment();
  if (env.kind === "local") return pickHostSettings(await getAllSettings());
  if (!hostSupports(env.protocol, "get_settings")) return null;
  return api.getSettings();
}

/** Re-read the host-owned settings from the engine the UI is now driving. Run
 *  at launch and on every switch and reconnect; an answer that lands after the
 *  user moved on is dropped. */
export async function hydrateHostSettings(set: Patch) {
  const settings = await forActiveEnvironment(readHostSettings);
  if (settings) applyHostSettings(set, settings);
}
