import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import { connectionDot } from "../../../lib/connection";
import { severalHosts, useApp, useEveryHost, useHostName, useHostStore } from "../../../store";
import { HostMenu } from "./HostMenu";

/** What the chip says: the one paired host's name and whether it is reachable,
 *  or how many hosts there are and whether every one of them is. */
function useChip(): { dot: string; label: string } {
  const several = useApp(severalHosts);
  const name = useHostName();
  const connection = useHostStore((s) => s.connection);
  const connections = useEveryHost((s) => s.connection);
  if (!several) return { dot: connectionDot(connection), label: name ?? "Host" };
  const allUp = connections.every((c) => c === "connected");
  return { dot: allUp ? "running" : "waiting", label: `${connections.length} hosts` };
}

/** The Home header's host chip, opening a dropdown of every paired host. It
 *  works the same with a host down — the dropdown is how the user reaches a
 *  host's details, or another host, past a Mac that is asleep. */
export function HostSwitcher() {
  const [open, setOpen] = useState(false);
  const { dot, label } = useChip();
  return (
    <>
      <button
        type="button"
        className="host"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className={`dot ${dot}`} />
        <span className="host-name">{label}</span>
        <Icon name="chevD" size={13} className="chev" />
      </button>
      {open && <HostMenu onClose={() => setOpen(false)} />}
    </>
  );
}
