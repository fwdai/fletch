import { useAppStore } from "@/store";
import { SetGroup, SetRow, SetToggle } from "../../primitives";
import type { Remote } from "../useRemote";
import { ConnectionAdvanced } from "./ConnectionAdvanced";
import { connectionProblem } from "./connectionProblem";

/** Whether this Mac lets paired devices in at all, and what stops them
 *  reaching it. The relay comes with the switch (the host turns it on with
 *  remote access) and only shows up here when it is in trouble. */
export function ConnectionGroup({ remote }: { remote: Remote }) {
  const { status, error, busy, setEnabled, setPort } = remote;
  const keepAwakeOnPower = useAppStore((s) => s.keepAwakeOnPower);
  const setKeepAwakeOnPower = useAppStore((s) => s.setKeepAwakeOnPower);
  const enabled = !!status?.enabled;
  const problem = connectionProblem(status);

  return (
    <SetGroup label="Connection">
      <SetRow
        title="Allow remote control"
        sub="Lets paired phones and Macs watch and steer agents on this Mac, from anywhere. End-to-end encrypted."
      >
        <SetToggle
          on={enabled}
          disabled={busy || !status}
          onClick={() => void setEnabled(!enabled)}
        />
      </SetRow>

      <SetRow
        title="Keep this Mac awake on power"
        sub="Stops idle sleep while plugged in, so your devices can reach this Mac. Closing the lid still sleeps it."
      >
        <SetToggle on={keepAwakeOnPower} onClick={() => setKeepAwakeOnPower(!keepAwakeOnPower)} />
      </SetRow>

      {/* The host's own standing problem (an unwritable device store, which
          also blocks pairing) first, then whatever the last command failed
          with, then what stops a device reaching this Mac. */}
      {status?.error && <div className="set-inline-warn">{status.error}</div>}
      {error && <div className="set-inline-warn">{error}</div>}
      {!status?.error && !error && problem && <div className="set-inline-warn">{problem}</div>}

      {status && (
        <ConnectionAdvanced port={status.port} disabled={busy} onSetPort={(p) => void setPort(p)} />
      )}
    </SetGroup>
  );
}
