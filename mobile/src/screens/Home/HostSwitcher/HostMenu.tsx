import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../../lib/ignore";
import { useStore } from "../../../store";
import { HostRow } from "./HostRow";

/** The dropdown under the host chip: every paired host, a tap away, then
 *  adding another and the active host's sheet, which is where it is unpaired.
 *  Every action closes it. */
export function HostMenu({ onClose }: { onClose: () => void }) {
  const hosts = useStore((s) => s.hosts);
  const activeKey = useStore((s) => s.hostKey);
  const connection = useStore((s) => s.connection);
  const switchHost = useStore((s) => s.switchHost);
  const addHost = useStore((s) => s.addHost);
  const openSheet = useStore((s) => s.openSheet);

  const then = (fn: () => void) => () => {
    onClose();
    fn();
  };

  return (
    <>
      <button type="button" className="host-menu-bg" aria-label="Close" onClick={onClose} />
      <div className="host-menu card fade" role="menu">
        {hosts.map((host) => (
          <HostRow
            key={host.hostKey}
            host={host}
            connection={host.hostKey === activeKey ? connection : null}
            onSelect={then(() => void switchHost(host.hostKey).catch(ignore))}
          />
        ))}
        <button type="button" className="row" role="menuitem" onClick={then(addHost)}>
          <span className="hm-mark">
            <Icon name="plus" size={16} />
          </span>
          <div className="main">
            <div className="lbl">Add a host…</div>
          </div>
        </button>
        <button
          type="button"
          className="row"
          role="menuitem"
          onClick={then(() => openSheet("host"))}
        >
          <span className="hm-mark">
            <Icon name="laptop" size={16} />
          </span>
          <div className="main">
            <div className="lbl">Host details</div>
          </div>
        </button>
      </div>
    </>
  );
}
