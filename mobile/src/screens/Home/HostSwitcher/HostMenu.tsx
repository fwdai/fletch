import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../../lib/ignore";
import { useApp, useHostNav } from "../../../store";
import { HostRow } from "./HostRow";

/** The dropdown under the host chip: every paired host, a tap away, then
 *  adding another and the sheet of the host on screen, which is where it is
 *  unpaired. Every action closes it. */
export function HostMenu({ onClose }: { onClose: () => void }) {
  const hosts = useApp((s) => s.hosts);
  const activeKey = useApp((s) => s.hostKey);
  const switchHost = useApp((s) => s.switchHost);
  const addHost = useApp((s) => s.addHost);
  const { openSheet } = useHostNav();

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
            active={host.hostKey === activeKey}
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
