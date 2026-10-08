import { Icon } from "@desktop/components/Icon";
import type { NearbyHost } from "../../remote/nearby";
import { Working } from "./Progress";

/** "Macs nearby": every Fletch host announcing itself on this network, by
 *  name, AirDrop style. Picking one is all the addressing pairing needs. */
export function NearbyList({
  hosts,
  searching,
  selected,
  disabled,
  onSelect,
}: {
  hosts: NearbyHost[];
  searching: boolean;
  /** The host key of the one picked, if any. */
  selected?: string;
  disabled?: boolean;
  onSelect: (host: NearbyHost) => void;
}) {
  return (
    <div className="field">
      <span className="label">Macs nearby</span>
      {hosts.length === 0 ? (
        <Working>
          {searching ? "Looking for Macs on this network…" : "No Macs found on this network yet…"}
        </Working>
      ) : (
        <div className="nearby">
          {hosts.map((host) => (
            <button
              key={host.hostKey}
              type="button"
              className="nearby-host"
              aria-pressed={host.hostKey === selected}
              disabled={disabled}
              onClick={() => onSelect(host)}
            >
              <Icon name="laptop" size={18} />
              <span>{host.name}</span>
              {host.hostKey === selected && <Icon name="check" size={16} />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
