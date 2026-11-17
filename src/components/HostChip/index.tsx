import { Badge } from "@/components/ui/Badge";
import { hostTone } from "@/remote/hostTone";
import { connectionLabel, useHostEntry } from "@/store/capabilities";
import { HostDot } from "./EnvironmentDot";

export { EnvironmentDot, HostDot } from "./EnvironmentDot";

/** Where something runs, when that is a paired host: the host's name beside
 *  its connection dot, for the host the calling component is scoped to. A host
 *  out of reach has the whole chip dimmed. On This Mac it is nothing, so a
 *  window with no paired host never shows one. */
export function HostChip() {
  const env = useHostEntry();
  if (env.kind === "local") return null;
  const tone = hostTone(env.connection, env.retrying);
  return (
    <Badge
      className={tone === "inactive" ? "host-chip inactive" : "host-chip"}
      tip={`Runs on ${env.name} · ${connectionLabel(env)}`}
    >
      <HostDot tone={tone} />
      <span className="host-chip-name">{env.name}</span>
    </Badge>
  );
}
