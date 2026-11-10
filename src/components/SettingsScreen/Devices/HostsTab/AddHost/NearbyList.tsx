import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NearbyHost } from "@/remote/nearby";
import { SetRow } from "../../../primitives";
import { AskingRow } from "./AskingRow";
import { CodeEntry } from "./CodeEntry";
import { hostOf, type NearbyStep } from "./nearbyPairing";

/** The Macs and hosts announcing themselves on this network, by name, AirDrop
 *  style. Picking one is all the addressing pairing needs, and the pairing
 *  then plays out on that host's row while the others wait. */
export function NearbyList({
  hosts,
  searching,
  step,
  onPick,
  onSubmitCode,
  onCancel,
}: {
  hosts: NearbyHost[];
  searching: boolean;
  step: NearbyStep;
  onPick: (host: NearbyHost) => void;
  onSubmitCode: (code: string) => void;
  onCancel: () => void;
}) {
  if (hosts.length === 0) {
    return searching ? (
      <SetRow title="Looking for Macs nearby…" />
    ) : (
      <SetRow
        title="No Macs found nearby"
        sub="On the other Mac, open Settings › Devices and turn on Allow remote control. Keep that page open while you pair."
      />
    );
  }
  const active = hostOf(step)?.hostKey;
  return hosts.map((host) =>
    host.hostKey !== active ? (
      <SetRow
        key={host.hostKey}
        icon={<Icon name="laptop" size={16} />}
        title={host.name}
        sub="On this network"
      >
        <Button variant="outline" size="sm" disabled={!!active} onClick={() => onPick(host)}>
          Pair
        </Button>
      </SetRow>
    ) : step.kind === "code" ? (
      <CodeEntry
        key={host.hostKey}
        host={host}
        sending={step.sending}
        onSubmit={onSubmitCode}
        onCancel={onCancel}
      />
    ) : (
      <AskingRow
        key={host.hostKey}
        host={host}
        code={step.kind === "asking" ? step.code : null}
        onCancel={onCancel}
      />
    ),
  );
}
