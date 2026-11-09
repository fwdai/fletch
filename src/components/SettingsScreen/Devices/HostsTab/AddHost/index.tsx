import type { SavedHost } from "@/storage/remoteHosts";
import { SetGroup } from "../../../primitives";
import type { AddHost as Add } from "../usePairedHosts";
import { AskingRow } from "./AskingRow";
import { CodeEntry } from "./CodeEntry";
import { NearbyList } from "./NearbyList";
import { unpairedNearby } from "./nearbyPairing";
import { PasteLink } from "./PasteLink";
import { useNearby } from "./useNearby";
import { useNearbyPairing } from "./useNearbyPairing";

/** Settings › Devices › Hosts › Add a host. The Macs announcing themselves on
 *  this network, live, each a click from pairing: the host's owner accepts
 *  when both screens show the same six digits, or, for a host nobody is
 *  sitting at, its code is typed here. A pasted link covers the hosts the
 *  list cannot see. Browsing runs only while this is mounted. */
export function AddHost({ paired, add }: { paired: SavedHost[]; add: Add }) {
  const nearby = useNearby();
  const pairing = useNearbyPairing(add);
  const { step } = pairing;

  return (
    <SetGroup label="Add a host">
      {step.kind === "asking" ? (
        <AskingRow host={step.host} code={step.code} onCancel={pairing.cancel} />
      ) : step.kind === "code" ? (
        <CodeEntry
          host={step.host}
          sending={step.sending}
          onSubmit={pairing.submitCode}
          onCancel={pairing.cancel}
        />
      ) : (
        <NearbyList
          hosts={unpairedNearby(nearby.hosts, paired)}
          searching={nearby.searching}
          onPick={pairing.pick}
        />
      )}

      {pairing.error && <div className="set-inline-warn">{pairing.error}</div>}

      <PasteLink add={add} />
    </SetGroup>
  );
}
