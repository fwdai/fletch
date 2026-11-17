import { Badge } from "@/components/ui/Badge";
import { connectionLabel, useHostEntry } from "@/store/capabilities";
import { EnvironmentDot } from "./EnvironmentDot";

export { EnvironmentDot } from "./EnvironmentDot";

/** Where something runs, when that is a paired host: the host's name beside
 *  its connection dot, for the host the calling component is scoped to. On
 *  This Mac it is nothing, so a window with no paired host never shows one. */
export function HostChip() {
  const env = useHostEntry();
  if (env.kind === "local") return null;
  return (
    <Badge className="host-chip" tip={`Runs on ${env.name} · ${connectionLabel(env)}`}>
      <EnvironmentDot env={env} />
      <span className="host-chip-name">{env.name}</span>
    </Badge>
  );
}
