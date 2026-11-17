import { Icon } from "@desktop/components/Icon";
import { useStore } from "zustand";
import { CONNECTION_TEXT, connectionLook, isInactive } from "../../../lib/connection";
import { hostStoreOf } from "../../../store";
import type { SavedHost } from "../../../store/persist";

/** One paired host in the host list, with its own connection, each host being
 *  connected on a link of its own; one out of reach is greyed. `checked` makes it a choice — the host on
 *  screen carrying the check — and leaving it out an action. */
export function HostRow({
  host,
  checked,
  onSelect,
}: {
  host: SavedHost;
  checked?: boolean;
  onSelect: () => void;
}) {
  const connection = useStore(hostStoreOf(host.hostKey), (s) => s.connection);
  const retrying = useStore(hostStoreOf(host.hostKey), (s) => s.retrying);
  return (
    <button
      type="button"
      className={isInactive(connection, retrying) ? "row inactive" : "row"}
      role={checked === undefined ? "menuitem" : "menuitemradio"}
      aria-checked={checked}
      onClick={onSelect}
    >
      <span className="hm-mark">
        <span className={`dot ${connectionLook(connection, retrying).dot}`} />
      </span>
      <div className="main">
        <div className="lbl">{host.name}</div>
        <div className="sub">{CONNECTION_TEXT[connection]}</div>
      </div>
      {checked && <Icon name="check" size={18} strokeWidth={2} className="check-ic" />}
    </button>
  );
}
