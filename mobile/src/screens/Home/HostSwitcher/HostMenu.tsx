import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../../lib/ignore";
import { useStore } from "../../../store";
import type { SavedHost } from "../../../store/persist";
import { HostRow } from "./HostRow";

/** Forgetting is not undone by anything on the phone — the Mac has to issue a
 *  new pairing — so it is asked, naming the host. */
const confirmForget = (host: SavedHost) =>
  typeof window === "undefined" ||
  window.confirm(`Forget ${host.name}? You'll need to pair with it again to reconnect.`);

/** The dropdown under the host chip: every paired host (tap to switch, the bin
 *  to forget), then adding another and the Host sheet. Every action closes it. */
export function HostMenu({ onClose }: { onClose: () => void }) {
  const hosts = useStore((s) => s.hosts);
  const activeKey = useStore((s) => s.hostKey);
  const connection = useStore((s) => s.connection);
  const switchHost = useStore((s) => s.switchHost);
  const forgetHost = useStore((s) => s.forgetHost);
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
            onForget={() => {
              if (confirmForget(host)) then(() => void forgetHost(host.hostKey).catch(ignore))();
            }}
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
