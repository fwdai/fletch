import type { AgentRecord, BoxStateEvent, CloudProviderStatus, LocalWorkspaceKind } from "@/api";
import type { IconName } from "@/components/Icon";
import { CLOUD_MACHINE, cloudMachineLabel, LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";
import { inCloud } from "@/util/workspaceKind";

/** How a workspace's agent is confined, as the title bar tells it apart. A
 *  box at a cloud provider is a `box` too: it has a box's phase and stop. */
export type IsolationKind = "box" | "container" | "seatbelt";

/** What a workspace's record says of where it runs. */
export type WorkspacePlace = Pick<AgentRecord, "sandbox_engine" | "cloud_provider">;

export interface Isolation {
  kind: IsolationKind;
  icon: IconName;
  /** What it is, in a few words: the popover's heading. */
  title: string;
  /** What that means for the agent and the user's machine. */
  hint: string;
}

// Each hint is at most two short lines in the popover: what the agent can
// reach and, for a machine at a cloud provider, what it costs; nothing else.
const BOX: Isolation = {
  kind: "box",
  icon: LINUX_VM.icon,
  title: LINUX_VM.label,
  hint: "Its own disk. Sees only folders you grant; reaches only allowed hosts.",
};

/** Seatbelt is the fallback: every engine the app does not run elsewhere. */
const SEATBELT: Isolation = {
  kind: "seatbelt",
  icon: MACOS_SANDBOX.icon,
  title: MACOS_SANDBOX.label,
  hint: "On your Mac. Writes only inside this workspace; personal folders stay closed.",
};

/** A retired engine's workspace, which no longer launches. */
function container(runtime: string): Isolation {
  return {
    kind: "container",
    icon: "container",
    title: `${runtime} container`,
    hint: "Fletch no longer runs containers. Start a new workspace.",
  };
}

/** A box on a machine at a cloud provider, named for the provider and its
 *  bill. `provider` is its entry in `cloudProviders`: absent until the store
 *  has read them, or for an id they do not list, when the words are neutral. */
function cloud(provider: CloudProviderStatus | undefined): Isolation {
  return {
    kind: "box",
    icon: CLOUD_MACHINE.icon,
    title: cloudMachineLabel(provider?.label),
    hint: `Its own machine; reaches any host. ${provider?.billing || "The provider bills your account."}`,
  };
}

/** The isolation of an agent's stamped `sandbox_engine` and `cloud_provider`;
 *  `providers` (the store's `cloudProviders`) names a cloud provider. */
export function isolationOf(
  place: WorkspacePlace,
  providers: CloudProviderStatus[] | null,
): Isolation {
  if (inCloud(place)) return cloud(providers?.find((p) => p.id === place.cloud_provider));
  if (place.sandbox_engine === "box") return BOX;
  if (place.sandbox_engine === "docker") return container("Docker");
  if (place.sandbox_engine === "podman") return container("Podman");
  return SEATBELT;
}

/** What the title bar shows for a workspace: where it runs, or, while it
 *  moves, where it's going (`movingTo`, as asked: the record is restamped
 *  before the move is over). A move stays on this Mac. */
export function shownIsolation(
  place: WorkspacePlace,
  movingTo: LocalWorkspaceKind | null,
  providers: CloudProviderStatus[] | null,
): Isolation {
  if (movingTo) return movingTo === "box" ? BOX : SEATBELT;
  return isolationOf(place, providers);
}

/** The box's phase as the title bar spells it. */
export const BOX_PHASE_LABEL: Record<BoxStateEvent["phase"], string> = {
  stopped: "Stopped",
  preparing: "Preparing",
  starting: "Starting",
  running: "Running",
  stopping: "Stopping",
  failed: "Failed",
};

/** Whether a box is up, and so something a user can stop. */
export function boxIsUp(phase: BoxStateEvent["phase"] | undefined): boolean {
  return phase === "running" || phase === "starting";
}

/** `Preparing · downloading image 43%`: a box waiting on the host's setup
 *  says what for, so a launch during the setup reads as late, not stuck. */
export function boxPhaseLabel(state: BoxStateEvent): string {
  const label = BOX_PHASE_LABEL[state.phase];
  return state.phase === "preparing" && state.detail ? `${label} · ${state.detail}` : label;
}
