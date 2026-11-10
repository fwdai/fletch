import { SetGroup, SetRow } from "../../primitives";
import type { Remote } from "../useRemote";
import { DeviceRow } from "./DeviceRow";

/** The devices allowed to control this Mac, each with its revoke. The page's
 *  last group while this Mac is not listening: there is no pairing window to
 *  show under it then. */
export function PairedDevices({ remote }: { remote: Remote }) {
  const { status, busy, revoke } = remote;
  const devices = status?.devices ?? [];

  return (
    <SetGroup label="Paired" last={!status?.listening}>
      {devices.length === 0 ? (
        <SetRow
          title="No clients yet"
          sub={
            status?.listening
              ? "Pair a phone or another Mac below."
              : "Turn on remote control to pair a phone or another Mac."
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
