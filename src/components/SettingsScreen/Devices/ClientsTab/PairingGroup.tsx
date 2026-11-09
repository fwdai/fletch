import type { PairingPreset } from "@/api";
import { Segmented } from "@/components/Settings/Segmented";
import { Button } from "@/components/ui/Button";
import { SetGroup, SetRow } from "../../primitives";
import type { Remote } from "../useRemote";
import { PairingCard } from "./PairingCard";
import { CONTROL_PRESET_HELP, PAIRING_PRESETS } from "./presets";

/** The pairing window the page keeps open (`usePairingWindow`): what the next
 *  device is granted, and the QR and code a phone pairs with. A Mac nearby
 *  needs neither — it asks, and the app-wide prompt takes it from there. Only
 *  while this Mac is listening: there is no window to show otherwise. */
export function PairingGroup({
  remote,
  preset,
  onPreset,
}: {
  remote: Remote;
  preset: PairingPreset;
  onPreset: (preset: PairingPreset) => void;
}) {
  const { status, invite, inviteClosed, beginPairing } = remote;
  if (!status?.listening) return null;

  return (
    <SetGroup label="Pair a device">
      <SetRow
        title="Access"
        sub={
          <>
            What the next device you pair may do.
            <br />
            {CONTROL_PRESET_HELP}
          </>
        }
        align="start"
      >
        <Segmented<PairingPreset> value={preset} options={PAIRING_PRESETS} onChange={onPreset} />
      </SetRow>

      {invite ? (
        <PairingCard
          invite={invite}
          hostName={status.name}
          closed={inviteClosed}
          lanOnly={status.relay.state !== "connected"}
          onRegenerate={() => void beginPairing(preset)}
        />
      ) : (
        // Only for a moment, or after a mint the host refused (the reason is
        // in the warning above): the window opens on its own.
        <SetRow title="Pairing code" sub="Not open yet.">
          <Button
            variant="outline"
            size="sm"
            disabled={!!status.error}
            onClick={() => void beginPairing(preset)}
          >
            New code
          </Button>
        </SetRow>
      )}
    </SetGroup>
  );
}
