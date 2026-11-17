import type { ForwardedPort } from "@/api";

/** A forwarded port as the VM knows it, with the port on this Mac beside it
 *  when the forward had to take another. */
export function PortLabel({ port }: { port: ForwardedPort }) {
  return (
    <>
      <span className="colon">:</span>
      <span className="port">{port.guest_port}</span>
      {port.port !== port.guest_port && <span className="mac-port">→ localhost:{port.port}</span>}
    </>
  );
}
