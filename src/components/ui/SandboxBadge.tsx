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

/** A subtle container chip shown next to the workspace path on containerized
 *  agents. Renders nothing for the default seatbelt engine, so it only appears
 *  when the sandbox engine is a container one (via `isContainerEngine`, the
 *  single definition of that). `engine` is the agent's stamped `sandbox_engine`
 *  value; both container runtimes share the chip's tone, because what it tells
 *  the user about their paths is the same either way. */
export function SandboxBadge({ engine }: { engine?: string | null }) {
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
