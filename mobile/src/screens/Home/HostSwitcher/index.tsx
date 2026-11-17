import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import { severalHosts, useApp, useEveryHost, useHostName } from "../../../store";
import { HostMenu } from "./HostMenu";

/** What the chip says: the one paired host's name, or how many hosts there
 *  are, and whether every one of them is connected. It is the way to the host
 *  list, not a host, so anything short of that is amber: which host is out of
 *  reach, and which has a problem, is the list's to say. */
function useChip(): { dot: string; label: string } {
  const several = useApp(severalHosts);
  const name = useHostName();
  const connections = useEveryHost((s) => s.connection);
  const allUp = connections.every((c) => c === "connected");
  return {
    dot: allUp ? "running" : "waiting",
    label: several ? `${connections.length} hosts` : (name ?? "Host"),
  };
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
