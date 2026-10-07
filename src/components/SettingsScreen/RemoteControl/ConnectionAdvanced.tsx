import type { RemoteStatus } from "@/api";
import { CopyButton } from "@/components/ui/CopyButton";
import { SetDisclosure, SetRow } from "../primitives";
import { PortRow } from "./PortRow";
import { RelayUrlRow } from "./RelayRow";

/** Settings › Remote control › Advanced: how the connection is plumbed. None
 *  of it is needed to pair or use a device — the pairing card shows the address
 *  when it is wanted — so it stays out of the way until asked for. */
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
  const addresses = status.listening
    ? status.addresses.map((a) => `${a}:${status.port}`).join(" · ") || "None found."
    : "Remote control is off.";

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
      <SetRow title="Local addresses" sub={<span className="mono">{addresses}</span>} />
      {status.hostId && (
        <SetRow
          title="Host ID"
          sub={
            <>
              This Mac's identity. A paired phone shows its start under Host › Advanced › Identity.
              <span className="set-host-id mono text-xs">{status.hostId}</span>
            </>
          }
          align="start"
        >
          <CopyButton text={status.hostId} tip="Copy host ID" />
        </SetRow>
      )}
    </SetDisclosure>
  );
}
