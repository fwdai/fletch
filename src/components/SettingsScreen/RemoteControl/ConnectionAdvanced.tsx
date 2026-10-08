import type { RemoteStatus } from "@/api";
import { SetDisclosure } from "../primitives";
import { PortRow } from "./PortRow";
import { RelayUrlRow } from "./RelayRow";

/** Settings › Remote control › Advanced: the two things someone running an
 *  unusual network may need to change. Only settings live here — read-only
 *  plumbing (addresses, keys) helps nobody, so it is not shown at all. */
export function ConnectionAdvanced({
  status,
  disabled,
  onSetPort,
  onSetRelay,
}: {
  status: RemoteStatus;
  disabled?: boolean;
  onSetPort: (port: number) => void;
  onSetRelay: (url: string) => void;
}) {
  return (
    <SetDisclosure label="Advanced">
      <PortRow port={status.port} disabled={disabled} onSet={onSetPort} />
      {status.relay.url !== null && (
        <RelayUrlRow
          relay={status.relay}
          disabled={disabled || !status.enabled}
          onSet={onSetRelay}
        />
      )}
    </SetDisclosure>
  );
}
