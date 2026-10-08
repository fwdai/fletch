import { useState } from "react";
import type { PairingPreset } from "@/api";
import { Segmented } from "@/components/Settings/Segmented";
import { Button } from "@/components/ui/Button";
import { SetGroup, SetHead, SetRow, SetToggle } from "../primitives";
import { ConnectionAdvanced } from "./ConnectionAdvanced";
import { DeviceRow } from "./DeviceRow";
import { PairedHosts } from "./PairedHosts";
import { PairingCard } from "./PairingCard";
import { CONTROL_PRESET_HELP, PAIRING_PRESETS } from "./presets";
import { RelayRow } from "./RelayRow";
import { useRemote } from "./useRemote";

/** Settings › Remote control: run the paired-device WebSocket server so a
 *  phone can drive the agents on this Mac.
 *
 *  Reachable on the LAN (or Tailscale) only, and every frame is end-to-end
 *  encrypted between the phone and this Mac — hence the plain statement of what
 *  the switch opens, and pairing that is an explicit, expiring, single-use act
 *  rather than a standing invitation. Ports, addresses and keys are under
 *  Advanced: nobody needs them to pair a phone. */
export function RemoteControlPane() {
  const {
    status,
    invite,
    inviteClosed,
    error,
    busy,
    setEnabled,
    setPort,
    setRelay,
    beginPairing,
    revoke,
    clearInvite,
  } = useRemote();

  // What the next code will grant. Full is the default, which is what every
  // pairing granted before presets existed; it is not remembered between
  // codes, so nothing narrows a pairing by accident.
  const [preset, setPreset] = useState<PairingPreset>("full");

  const enabled = !!status?.enabled;
  const listening = !!status?.listening;
  const devices = status?.devices ?? [];

  // Only what stops a phone from reaching this Mac; a healthy connection says
  // nothing beyond its switches.
  const problem = listening
    ? status?.addresses.length === 0
      ? "No network address found — connect this Mac to Wi-Fi or Ethernet."
      : null
    : enabled
      ? "On, but the port could not be opened."
      : null;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Remote control"
        title="Remote control"
        desc="Watch and steer your agents from Fletch on your phone. Turn on the connection, pair a device with a one-time code, and manage the devices that can reach this Mac. This Mac can also be paired with another machine running Fletch, so its agents are reachable from here."
      />

      <SetGroup label="Connection">
        <SetRow
          title="Allow remote control"
          sub="Lets a paired phone watch and steer agents over your network. End-to-end encrypted."
        >
          <SetToggle
            on={enabled}
            disabled={busy || !status}
            onClick={() => void setEnabled(!enabled)}
          />
        </SetRow>

        {status && (
          <RelayRow
            relay={status.relay}
            disabled={busy || !enabled}
            onSet={(url) => void setRelay(url)}
          />
        )}

        {/* The host's own standing problem (an unwritable device store, which
            also blocks pairing) first, then whatever the last command failed
            with, then what is wrong with the listener. */}
        {status?.error && <div className="set-inline-warn">{status.error}</div>}
        {error && <div className="set-inline-warn">{error}</div>}
        {!status?.error && !error && problem && <div className="set-inline-warn">{problem}</div>}

        {status && (
          <ConnectionAdvanced
            status={status}
            disabled={busy}
            onSetPort={(p) => void setPort(p)}
            onSetRelay={(url) => void setRelay(url)}
          />
        )}
      </SetGroup>

      <SetGroup label="Devices">
        {listening && !invite && (
          <SetRow
            title="Pair a device"
            sub={
              <>
                Opens pairing for five minutes: scan the QR code with your phone, or pick this Mac
                in Fletch on your phone and accept here.
                <br />
                {CONTROL_PRESET_HELP}
              </>
            }
            align="start"
          >
            <Segmented<PairingPreset>
              value={preset}
              options={PAIRING_PRESETS}
              onChange={setPreset}
            />
            <Button
              variant="primary"
              disabled={!!status?.error}
              onClick={() => void beginPairing(preset)}
            >
              Pair a device
            </Button>
          </SetRow>
        )}

        {invite && (
          <PairingCard
            invite={invite}
            hostName={status?.name}
            closed={inviteClosed}
            lanOnly={status?.relay.state !== "connected"}
            onRegenerate={() => void beginPairing(invite.preset)}
            onDismiss={clearInvite}
          />
        )}

        {devices.length === 0 ? (
          <SetRow
            title="Paired devices"
            sub={listening ? "None yet." : "None yet. Turn on remote control to pair a device."}
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

      <PairedHosts />
    </div>
  );
}
