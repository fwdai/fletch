import type { ForwardedPort } from "@/api";
import { OtherPorts } from "./OtherPorts";
import { PortLink } from "./PortLink";

interface Props {
  ports: ForwardedPort[];
  /** The run command's own guest port, which the bar already links. */
  runGuestPort: number | null;
}

/** The VM's forwarded ports besides the run command's own, under the Run bar:
 *  a link for each port the run listens on, and the rest (a server the agent
 *  started, a container's published port) behind one menu. */
export function RunPorts({ ports, runGuestPort }: Props) {
  const listed = ports.filter((p) => p.guest_port !== runGuestPort);
  if (listed.length === 0) return null;
  const others = listed.filter((p) => !p.run);
  return (
    <div className="run-bar v2 run-ports">
      {listed
        .filter((p) => p.run)
        .map((p) => (
          <PortLink key={p.guest_port} port={p} />
        ))}
      {others.length > 0 && <OtherPorts ports={others} />}
    </div>
  );
}
