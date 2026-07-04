import { Badge } from "./Badge";

/** A small "Docker" chip shown on containerized agents (header + sidebar row).
 *  Renders nothing for seatbelt agents (the default), so the sandbox engine is
 *  only surfaced when it's the non-default one. `engine` is the agent's stamped
 *  `sandbox_engine` value. */
export function EngineBadge({ engine }: { engine?: string | null }) {
  if (engine !== "docker") return null;
  return (
    <Badge variant="docker" tip="Runs in a Docker container (Linux)">
      Docker
    </Badge>
  );
}
