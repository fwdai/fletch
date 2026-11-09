import { SetGroup, SetRow } from "../../primitives";
import type { Remote } from "../useRemote";
import { DeviceRow } from "./DeviceRow";

/** The devices allowed to control this Mac, each with its revoke. */
export function PairedDevices({ remote }: { remote: Remote }) {
  const { status, busy, revoke } = remote;
  const devices = status?.devices ?? [];

  return (
    <SetGroup label="Paired devices" last>
      {devices.length === 0 ? (
        <SetRow
          title="Devices"
          sub={
            status?.listening
              ? "None yet."
              : "None yet. Turn on remote control to pair a phone or another Mac."
          }
        />
      ) : (
        devices.map((device) => (
          <DeviceRow
            key={device.deviceId}
            device={device}
            disabled={busy}
            onRevoke={() => void revoke(device.deviceId)}
          />
        ))
      )}
    </SetGroup>
  );
}
