// The host-owned global settings, as the store holds them: read from each
// host's engine, folded into the slices' fields in that host's partition, and
// kept live by its `settings:changed`. The local environment reads its own
// `settings` table (the generic bridge it always used); a remote one asks the
// host with `get_settings`, since its values are the host's and live in the
// host's database (docs/remote-protocol.md, "Settings").

import type { SettingsChangedEvent } from "@/api";
import type { Engine } from "@/api/engine";
import { hostSupports } from "@/remote/types";
import { pickHostSettings } from "@/storage/hostOwnedKeys";
import {
  parseAutoArchiveIdleDays,
  parseProviderPathOverrides,
  parsePublishApprovalWait,
} from "@/storage/preferences";
import { getAllSettings } from "@/storage/settings";
import { newestWins } from "@/util/newestWins";
import { replaySettingChanges, withSetting } from "@/util/settingsReplay";
import { type EnvironmentId, environmentEntry } from "./environments";
import { perHost } from "./perHost";
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
    // Opt-in: the project context layer is piloted behind Settings › Developer.
    contextLayerEnabled: s.context_layer_enabled === "true",
    // Opt-in, unlike the alerts: autopilot publishes unattended, so defaulting
    // it on would hang every unattended run until the decision timeout.
    publishConfirmation: s.publish_confirmation === "true",
    publishApprovalWait: parsePublishApprovalWait(s.publish_approval_wait),
    autoArchiveIdleDays: parseAutoArchiveIdleDays(s.auto_archive_idle_days),
    branchPrefix: s.git_branch_prefix || "",
    draftPrs: s.github_draft_prs === "true",
    // The box knobs are blank when unset (the engine's defaults apply).
    boxMemoryMib: s.box_memory_mib || "",
    boxCpus: s.box_cpus || "",
    boxDiskGib: s.box_disk_gib || "",
    boxImageUrl: s.box_image_url || "",
    boxIdleStopMinutes: s.box_idle_stop_minutes || "",
    boxEgressMode: s.box_egress_mode || "",
  };
}

/** One host's settings as this window holds them. */
interface HostSettingsState {
  /** What the store last read for the host — the base a `settings:changed`
   *  event is folded over, since one key can decide a field that reads
   *  several (`providerPathOverrides`). */
  current: Record<string, string>;
  /** One order for every hydrate: two reconnects issue two reads of the same
   *  host, and the older answer must not land over the newer. */
  hydrates: ReturnType<typeof newestWins>;
  /** The `settings:changed` events that landed while the newest hydrate's read
   *  was in flight, replayed over its answer: the host may have served the
   *  read before a write the event already put on screen. */
  inFlight: SettingsChangedEvent[] | null;
}

const settingsOf = perHost(
  (): HostSettingsState => ({ current: {}, hydrates: newestWins(), inFlight: null }),
);

export function applyHostSettings(
  hostId: EnvironmentId,
  set: Patch,
  settings: Record<string, string>,
) {
  const host = settingsOf(hostId);
  host.current = { ...settings };
  set(hostSettingsState(host.current));
}

/** Fold one write — this window's, another desktop's, a phone's — over what was
 *  read from `hostId`, without a refetch. */
export function applyHostSettingChange(hostId: EnvironmentId, set: Patch, e: SettingsChangedEvent) {
  const host = settingsOf(hostId);
  host.inFlight?.push(e);
  applyHostSettings(hostId, set, withSetting(host.current, e.key, e.value));
}

/** The engine's host-owned settings, or `null` when it cannot say: a host from
 *  before `get_settings`, whose Settings controls are gated closed anyway
 *  (`hostSettings` / `publishSettings`), keeps whatever was on screen. */
async function readHostSettings({
  api,
  hostId,
}: Pick<Engine, "api" | "hostId">): Promise<Record<string, string> | null> {
  const env = environmentEntry(hostId);
  if (env.kind === "local") return pickHostSettings(await getAllSettings());
  if (!hostSupports(env.protocol, "get_settings")) return null;
  return api.getSettings();
}

/** Re-read the host-owned settings from `engine`'s host. Run at launch and on
 *  every handshake; an answer older than a later read of the same host is
 *  dropped. */
export async function hydrateHostSettings(engine: Pick<Engine, "api" | "hostId">, set: Patch) {
  const host = settingsOf(engine.hostId);
  const claim = host.hydrates.claim();
  const buffer: SettingsChangedEvent[] = [];
  host.inFlight = buffer;
  try {
    const settings = await readHostSettings(engine);
    if (!settings || !claim.current()) return;
    applyHostSettings(engine.hostId, set, replaySettingChanges(settings, buffer));
  } finally {
    if (host.inFlight === buffer) host.inFlight = null;
  }
}
