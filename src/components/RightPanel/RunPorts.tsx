import type { ForwardedPort } from "@/api";
import { Icon } from "@/components/Icon";

interface Props {
  ports: ForwardedPort[];
  /** The run command's own guest port, which the bar already links. */
  runGuestPort: number | null;
}

/** The VM's other forwarded ports, one link each, under the Run bar. */
export function RunPorts({ ports, runGuestPort }: Props) {
  const others = ports.filter((p) => p.guest_port !== runGuestPort);
  if (others.length === 0) return null;
  return (
    <div className="run-bar v2 run-ports">
      {others.map((p) => (
        <a
          key={p.guest_port}
          href={`http://localhost:${p.port}`}
          target="_blank"
          rel="noreferrer"
          className="run-link text-xs"
        >
          <span className="colon">:</span>
          <span className="port">{p.guest_port}</span>
          {p.port !== p.guest_port && <span className="mac-port">→ localhost:{p.port}</span>}
          <Icon name="external" size={10} />
        </a>
      ))}
    </div>
  );
}
