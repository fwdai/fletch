import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import { connectionDot } from "../../../lib/connection";
import { activeHostName, useStore } from "../../../store";
import { HostMenu } from "./HostMenu";

/** The Home header's host chip: the active host and whether it is reachable,
 *  opening a dropdown of every paired host to switch between. It works the
 *  same with the active host down — switching away is how the user gets past
 *  a Mac that is asleep. */
export function HostSwitcher() {
  const [open, setOpen] = useState(false);
  const name = useStore(activeHostName);
  const connection = useStore((s) => s.connection);
  return (
    <>
      <button
        type="button"
        className="host"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <span className={`dot ${connectionDot(connection)}`} />
        <span className="host-name">{name ?? "Host"}</span>
        <Icon name="chevD" size={13} className="chev" />
      </button>
      {open && <HostMenu onClose={() => setOpen(false)} />}
    </>
  );
}
