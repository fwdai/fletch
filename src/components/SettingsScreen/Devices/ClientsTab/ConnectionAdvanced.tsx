import { SetDisclosure } from "../../primitives";
import { PortRow } from "./PortRow";

/** Settings › Devices › Clients › Advanced: the one thing someone running an
 *  unusual network may need to change. Only settings live here — read-only
 *  plumbing (addresses, keys, the relay) helps nobody, so it is not shown. */
export function ConnectionAdvanced({
  port,
  disabled,
  onSetPort,
}: {
  /** The port the host reports — bound when listening, configured otherwise. */
  port: number;
  disabled?: boolean;
  onSetPort: (port: number) => void;
}) {
  return (
    <SetDisclosure label="Advanced">
      <PortRow port={port} disabled={disabled} onSet={onSetPort} />
    </SetDisclosure>
  );
}
