import { Icon } from "@/components/Icon";
import { Badge } from "./Badge";

/** Why the path looks like the host: docker agents bind-mount the workspace at
 *  its exact host path ("path identity"), so `find`/diff output shows
 *  `/Users/.../worktrees/…` even though the process is confined to the
 *  container. This tooltip makes that non-obvious design legible. */
const TIP =
  "Runs inside a Docker container. Paths mirror your host exactly by design, but the agent is confined to its mounted workspace — it can't reach the rest of your machine.";

/** A subtle container chip shown next to the workspace path on containerized
 *  agents. Renders nothing for the default seatbelt engine, so it only appears
 *  when the sandbox engine is the non-default one. `engine` is the agent's
 *  stamped `sandbox_engine` value. */
export function SandboxBadge({ engine }: { engine?: string | null }) {
  if (engine !== "docker") return null;
  return (
    <Badge variant="docker" tip={TIP} tipDown className="tip-wide sandbox-badge">
      <Icon name="cube" size={10} />
    </Badge>
  );
}
