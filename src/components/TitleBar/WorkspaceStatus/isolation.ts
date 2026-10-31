import type { BoxStateEvent } from "@/api";
import type { IconName } from "@/components/Icon";
import type { BoxSetup } from "@/components/SettingsScreen/Sandbox/boxSetup";

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

const BOX: Isolation = {
  kind: "box",
  icon: "cube",
  title: "Isolated VM",
  hint: "Runs inside a micro-VM reachable only over SSH. The box shares no folder with your Mac and reaches only the project's allowed hosts.",
};

/** Seatbelt is the fallback: every engine the app does not run elsewhere. */
const SEATBELT: Isolation = {
  kind: "seatbelt",
  icon: "folderLock",
  title: "macOS sandbox",
  hint: "Runs on your Mac under the macOS sandbox (Seatbelt). It writes only inside this workspace, and your Downloads, Desktop, Documents and iCloud Drive stay closed to it.",
};

/** Container agents bind-mount the workspace at its exact host path ("path
 *  identity"), so their paths look like the host's even though the process is
 *  confined to the container — said here, since it is not obvious. */
function container(runtime: string): Isolation {
  return {
    kind: "container",
    icon: "container",
    title: `${runtime} container`,
    hint: `Runs inside a ${runtime} container. Paths mirror your host exactly by design, but the agent is confined to its mounted workspace — it can't reach the rest of your machine.`,
  };
}

/** The isolation of an agent's stamped `sandbox_engine`. */
export function isolationOf(engine: string | null | undefined): Isolation {
  if (engine === "box") return BOX;
  if (engine === "docker") return container("Docker");
  if (engine === "podman") return container("Podman");
  return SEATBELT;
}

/** One more line under a seatbelt workspace's hint: where the isolated VM
 *  stands, for a host that could run one. The workspace itself stays where
 *  it is — the line is about the next one — so it says so. `placement` is
 *  the project's stored `sandbox.placement`; a project kept on this Mac is
 *  the whole answer. Null when there is nothing worth adding: the switch
 *  off, a machine that cannot boot a box, a setup that failed (Settings has
 *  that), or no answer yet. */
export function seatbeltNote(
  vmUsable: boolean,
  setup: BoxSetup,
  placement: string | undefined,
): string | null {
  if (placement === "host") {
    return "This project's workspaces run on your Mac (Project Settings › Sandbox).";
  }
  if (!vmUsable) return null;
  switch (setup.state) {
    case "ready":
      return "Isolated VMs are ready. New workspaces run in one.";
    case "busy":
      return `An isolated VM is being prepared — ${setup.text.replace(/…$/, "").toLowerCase()}. New workspaces will run in it.`;
    default:
      return null;
  }
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
 *  says what for, so a launch right after the flag went on reads as late,
 *  not stuck. */
export function boxPhaseLabel(state: BoxStateEvent): string {
  const label = BOX_PHASE_LABEL[state.phase];
  return state.phase === "preparing" && state.detail ? `${label} · ${state.detail}` : label;
}
