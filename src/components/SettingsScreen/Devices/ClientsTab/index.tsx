import type { PairingPreset } from "@/api";
import type { Remote } from "../useRemote";
import { ConnectionGroup } from "./ConnectionGroup";
import { PairedDevices } from "./PairedDevices";
import { PairingGroup } from "./PairingGroup";

/** Settings › Devices › Clients: the devices allowed to control this Mac —
 *  this Mac *as a host*. Whether it lets them in, the ones already paired, and
 *  the pairing window a new one comes through. The server state is the page's
 *  (`useRemote`), since the pairing window stays open on either tab. */
export function ClientsTab({
  remote,
  preset,
  onPreset,
}: {
  remote: Remote;
  preset: PairingPreset;
  onPreset: (preset: PairingPreset) => void;
}) {
  return (
    <>
      <ConnectionGroup remote={remote} />
      <PairedDevices remote={remote} />
      <PairingGroup remote={remote} preset={preset} onPreset={onPreset} />
    </>
  );
}
