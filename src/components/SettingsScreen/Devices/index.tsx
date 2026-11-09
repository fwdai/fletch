import { useState } from "react";
import type { PairingPreset } from "@/api";
import { useAppStore } from "@/store";
import { SetHead } from "../primitives";
import { ClientsTab } from "./ClientsTab";
import { DevicesSwitch } from "./DevicesSwitch";
import { HostsTab } from "./HostsTab";
import { usePairingWindow } from "./usePairingWindow";
import { useRemote } from "./useRemote";

/** Settings › Devices: this Mac on both ends of remote control. Hosts are the
 *  machines it controls; Clients are the phones and Macs allowed to control
 *  it. Every connection either way is end-to-end encrypted.
 *
 *  One pane behind both tabs, so the pairing window it keeps open
 *  (`usePairingWindow`) lasts as long as the page rather than the tab: a Mac
 *  nearby can ask to pair whichever tab is showing, and the app-wide prompt
 *  takes it from there. */
export function DevicesPane() {
  const section = useAppStore((s) => s.settingsSection);
  const remote = useRemote();
  // What the next code will grant. Full is the default, which is what every
  // pairing granted before presets existed; it is not remembered between
  // visits, so nothing narrows a pairing by accident.
  const [preset, setPreset] = useState<PairingPreset>("full");
  usePairingWindow(remote, preset);
  const clients = section === "clients";

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Devices"
        eyebrowAside={<DevicesSwitch />}
        title={clients ? "Clients" : "Hosts"}
        desc={
          clients
            ? "Phones and Macs that can control this Mac."
            : "Macs and hosts this Mac can control."
        }
      />
      {clients ? <ClientsTab remote={remote} preset={preset} onPreset={setPreset} /> : <HostsTab />}
    </div>
  );
}
