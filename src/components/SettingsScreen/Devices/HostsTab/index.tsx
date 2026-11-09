import { SetGroup, SetRow } from "../../primitives";
import { AddHost } from "./AddHost";
import { HostRow } from "./HostRow";
import { usePairedHosts } from "./usePairedHosts";

/** Settings › Devices › Hosts: the machines this Mac controls — this Mac *as
 *  a client*. A host added here is dialled in the background from then on, at
 *  every launch, and the environment switcher offers it; one that is offline
 *  costs nothing but its own dot. With none, this Mac only runs its own
 *  agents and nothing here dials anything. */
export function HostsTab() {
  const { hosts, add, forget, busy } = usePairedHosts();

  return (
    <>
      <AddHost paired={hosts} add={add} />

      <SetGroup label="Hosts" last>
        {hosts.length === 0 ? (
          <SetRow title="Hosts" sub="None yet. This Mac only runs its own agents." />
        ) : (
          hosts.map((host) => (
            <HostRow
              key={host.hostKey}
              host={host}
              disabled={busy}
              onForget={() => void forget(host.hostKey)}
            />
          ))
        )}
      </SetGroup>
    </>
  );
}
