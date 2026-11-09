import { Icon } from "@desktop/components/Icon";
import { CONNECTION_TEXT, connectionDot } from "../../../lib/connection";
import type { ConnectionState } from "../../../remote";
import type { SavedHost } from "../../../store/persist";

/** One paired host in the switcher. `connection` is set for the active host
 *  only — the one link there is — and drives its dot and status line; the
 *  others show a hollow dot, since nothing is known about them until they are
 *  switched to. */
export function HostRow({
  host,
  connection,
  onSelect,
  onForget,
}: {
  host: SavedHost;
  connection: ConnectionState | null;
  onSelect: () => void;
  onForget: () => void;
}) {
  const active = connection !== null;
  return (
    <div className="row hm-host">
      <button
        type="button"
        className="hm-pick"
        role="menuitemradio"
        aria-checked={active}
        onClick={onSelect}
      >
        <span className="hm-mark">
          <span className={`dot ${active ? connectionDot(connection) : "idle"}`} />
        </span>
        <div className="main">
          <div className="lbl">{host.name}</div>
          {active && <div className="sub">{CONNECTION_TEXT[connection]}</div>}
        </div>
        {active && <Icon name="check" size={18} strokeWidth={2} className="check-ic" />}
      </button>
      <button
        type="button"
        className="hm-forget"
        aria-label={`Forget ${host.name}`}
        onClick={onForget}
      >
        <Icon name="trash" size={15} />
      </button>
    </div>
  );
}
