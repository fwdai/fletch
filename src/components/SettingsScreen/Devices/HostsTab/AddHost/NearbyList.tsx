import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NearbyHost } from "@/remote/nearby";
import { SetRow } from "../../../primitives";

/** The Macs and hosts announcing themselves on this network, by name, AirDrop
 *  style. Picking one is all the addressing pairing needs. */
export function NearbyList({
  hosts,
  searching,
  onPick,
}: {
  hosts: NearbyHost[];
  searching: boolean;
  onPick: (host: NearbyHost) => void;
}) {
  if (hosts.length === 0) {
    return (
      <SetRow
        title="Nearby"
        sub={
          searching
            ? "Looking for Macs on this network…"
            : "No other Macs found on this network. A Mac shows up here while remote control is on in its Settings › Devices › Clients."
        }
      />
    );
  }
  return hosts.map((host) => (
    <SetRow
      key={host.hostKey}
      icon={<Icon name="laptop" size={16} />}
      title={host.name}
      sub="On this network"
    >
      <Button variant="outline" size="sm" onClick={() => onPick(host)}>
        Pair
      </Button>
    </SetRow>
  ));
}
