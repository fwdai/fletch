import type { BoxStateEvent } from "@/api";
import { Icon } from "@/components/Icon";
import { isContainerEngine } from "@/storage/preferences";
import { Badge } from "./Badge";

/** Why the path looks like the host: container agents bind-mount the workspace
 *  at its exact host path ("path identity"), so `find`/diff output shows
 *  `/Users/.../workspaces/…` even though the process is confined to the
 *  container. This explanation makes that non-obvious design legible. It rides
 *  on the native `title` tooltip (via Badge's `hint`) so the OS positions it and
 *  it can't clip at a window edge like a hand-placed CSS tooltip would. */
function hint(runtime: string) {
  return `Runs inside a ${runtime} container. Paths mirror your host exactly by design, but the agent is confined to its mounted workspace — it can't reach the rest of your machine.`;
}

const BOX_HINT =
  "Runs inside a micro-VM reachable only over SSH. The box shares no folder with your Mac and reaches only the project's allowed hosts.";

/** The box's phase as the chip spells it (docs/boxes-spec.md §9). */
const BOX_PHASE_LABEL: Record<BoxStateEvent["phase"], string> = {
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
function phaseLabel(state: BoxStateEvent): string {
  const label = BOX_PHASE_LABEL[state.phase];
  return state.phase === "preparing" && state.detail ? `${label} · ${state.detail}` : label;
}

/** A subtle sandbox chip shown next to the workspace path on containerized and
 *  box agents. Renders nothing for the default seatbelt engine. `engine` is the
 *  agent's stamped `sandbox_engine` value; both container runtimes share the
 *  chip's tone, because what it tells the user about their paths is the same
 *  either way. A box chip also carries the box's live phase (`boxState`, from
 *  `box:state`), since a box can be stopped while its agent is still open. */
export function SandboxBadge({
  engine,
  boxState,
}: {
  engine?: string | null;
  boxState?: BoxStateEvent | null;
}) {
  if (engine === "box") {
    const phase = boxState ? phaseLabel(boxState) : null;
    return (
      <Badge
        variant={boxState?.phase === "failed" ? "err" : "docker"}
        label={phase ? `Box sandbox · ${phase}` : "Box sandbox"}
        hint={boxState?.reason ?? BOX_HINT}
        className="sandbox-badge"
      >
        <Icon name="cube" size={10} />
        {phase && <span>{phase}</span>}
      </Badge>
    );
  }
  if (!isContainerEngine(engine)) return null;
  const runtime = engine === "podman" ? "Podman" : "Docker";
  return (
    <Badge
      variant="docker"
      label={`${runtime} sandbox`}
      hint={hint(runtime)}
      className="sandbox-badge"
    >
      <Icon name="cube" size={10} />
    </Badge>
  );
}
