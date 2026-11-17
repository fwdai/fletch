import { Icon } from "@desktop/components/Icon";
import { useStore } from "zustand";
import { CONNECTION_TEXT, connectionDot } from "../../../lib/connection";
import { hostStoreOf } from "../../../store";
import type { SavedHost } from "../../../store/persist";

/** One paired host in the switcher. Its dot is its own connection, each host
 *  being connected on a link of its own; the host on screen (`active`) also
 *  spells it out, and carries the check. */
export function HostRow({
  host,
  active,
  onSelect,
}: {
  host: SavedHost;
  active: boolean;
  onSelect: () => void;
}) {
  const connection = useStore(hostStoreOf(host.hostKey), (s) => s.connection);
  return (
    <button
      type="button"
      className="row"
      role="menuitemradio"
      aria-checked={active}
      onClick={onSelect}
    >
      <span className="hm-mark">
        <span className={`dot ${connectionDot(connection)}`} />
      </span>
      <div className="main">
        <div className="lbl">{host.name}</div>
        {active && <div className="sub">{CONNECTION_TEXT[connection]}</div>}
      </div>
      {active && <Icon name="check" size={18} strokeWidth={2} className="check-ic" />}
    </button>
  );
}
