import { Button } from "@/components/ui/Button";
import { SetGroup, SetRow, SetToggle } from "../primitives";
import { DeviceRow } from "./DeviceRow";
import { PairingCard } from "./PairingCard";
import { useRemote } from "./useRemote";

/** Settings › General › Mobile devices: run the paired-device WebSocket server
 *  so a phone can drive the agents on this Mac.
 *
 *  Reachable on the LAN (or Tailscale) only, and every frame is end-to-end
 *  encrypted between the phone and this Mac — hence the plain statement of what
 *  the switch opens, and pairing that is an explicit, expiring, single-use act
 *  rather than a standing invitation. */
export function MobileDevices() {
  const { status, invite, error, busy, setEnabled, beginPairing, revoke, clearInvite } =
    useRemote();

  const enabled = !!status?.enabled;
  const listening = !!status?.listening;
  const devices = status?.devices ?? [];
  const addresses = status?.addresses ?? [];

  const reach = listening
    ? addresses.length > 0
      ? addresses.map((a) => `${a}:${status?.port}`).join(" · ")
      : "No network address found — connect this Mac to Wi-Fi or Ethernet."
    : enabled
      ? "On, but the port could not be opened."
      : "Off. No port is open.";

  return (
    <SetGroup label="Mobile devices">
      <SetRow
        title="Remote control from your phone"
        sub="Opens a local WebSocket port so a paired phone can watch and steer your agents. Reachable on your own Wi-Fi or a Tailscale network; every frame is encrypted end to end, and only paired devices are ever answered."
      >
        <SetToggle
          on={enabled}
          disabled={busy || !status}
          onClick={() => void setEnabled(!enabled)}
        />
      </SetRow>

      <SetRow title="Reachable at" sub={reach} align="start">
        {listening && <span className="set-remote-port mono text-sm">port {status?.port}</span>}
      </SetRow>

      {/* The host's own standing problem (an unwritable device store, which
          also blocks pairing) first, then whatever the last command failed
          with. */}
      {status?.error && <div className="set-inline-warn">{status.error}</div>}
      {error && <div className="set-inline-warn">{error}</div>}

      {listening && !invite && (
        <SetRow
          title="Pair a device"
          sub="Generates a one-time code, good for five minutes. Enter it in Fletch on your phone."
        >
          <Button variant="primary" disabled={!!status?.error} onClick={() => void beginPairing()}>
            Pair a device
          </Button>
        </SetRow>
      )}

      {invite && (
        <PairingCard
          invite={invite}
          hostId={status?.hostId}
          onRegenerate={() => void beginPairing()}
          onDismiss={clearInvite}
        />
      )}

      {devices.length === 0 ? (
        <SetRow title="Paired devices" sub="None yet." />
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
