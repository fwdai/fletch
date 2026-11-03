import type { BoxStateEvent } from "@/api";
import type { IconName } from "@/components/Icon";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";

/** How a workspace's agent is confined, as the title bar tells it apart. */
export type IsolationKind = "box" | "container" | "seatbelt";

export interface Isolation {
  kind: IsolationKind;
  icon: IconName;
  /** What it is, in a few words: the popover's heading. */
  title: string;
  /** What that means for the agent and the user's machine. */
  hint: string;
}

// Each hint is at most two short lines in the popover: what the agent can
// reach, nothing else.
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

/** The isolation of an agent's stamped `sandbox_engine`. */
export function isolationOf(engine: string | null | undefined): Isolation {
  if (engine === "box") return BOX;
  if (engine === "docker") return container("Docker");
  if (engine === "podman") return container("Podman");
  return SEATBELT;
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

/** `Preparing · downloading image 43%`: a box waiting on the host's setup
 *  says what for, so a launch during the setup reads as late, not stuck. */
export function boxPhaseLabel(state: BoxStateEvent): string {
  const label = BOX_PHASE_LABEL[state.phase];
  return state.phase === "preparing" && state.detail ? `${label} · ${state.detail}` : label;
}
