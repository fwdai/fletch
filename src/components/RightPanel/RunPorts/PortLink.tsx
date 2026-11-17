import type { ForwardedPort } from "@/api";
import { Icon } from "@/components/Icon";
import { PortLabel } from "./PortLabel";

/** The address on this Mac that reaches a forwarded port. */
export function portUrl(port: ForwardedPort): string {
  return `http://localhost:${port.port}`;
}

/** One forwarded port as a link in the Run bar. */
export function PortLink({ port }: { port: ForwardedPort }) {
  return (
    <a href={portUrl(port)} target="_blank" rel="noreferrer" className="run-link text-xs">
      <PortLabel port={port} />
      <Icon name="external" size={10} />
    </a>
  );
}
