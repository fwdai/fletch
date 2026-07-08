import { Icon } from "@/components/Icon";
import { Badge } from "./Badge";

const NAME = "Docker sandbox";

/** Why the path looks like the host: docker agents bind-mount the workspace at
 *  its exact host path ("path identity"), so `find`/diff output shows
 *  `/Users/.../workspaces/…` even though the process is confined to the
 *  container. This explanation makes that non-obvious design legible. It rides
 *  on the native `title` tooltip (via Badge's `hint`) so the OS positions it and
 *  it can't clip at a window edge like a hand-placed CSS tooltip would. */
const HINT =
  "Runs inside a Docker container. Paths mirror your host exactly by design, but the agent is confined to its mounted workspace — it can't reach the rest of your machine.";

/** A subtle container chip shown next to the workspace path on containerized
 *  agents. Renders nothing for the default seatbelt engine, so it only appears
 *  when the sandbox engine is the non-default one. `engine` is the agent's
 *  stamped `sandbox_engine` value. */
export function SandboxBadge({ engine }: { engine?: string | null }) {
  if (engine !== "docker") return null;
  return (
    <Badge variant="docker" label={NAME} hint={HINT} className="sandbox-badge">
      <Icon name="cube" size={10} />
    </Badge>
  );
}
