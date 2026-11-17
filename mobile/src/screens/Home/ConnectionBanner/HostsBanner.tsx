import { Notice } from "../../../components/ui/Notice";
import { ignore } from "../../../lib/ignore";
import type { ConnectionState } from "../../../remote";
import { hostStoreOf, useApp, useEveryHost } from "../../../store";

const dialling = (c: ConnectionState) => c === "connecting" || c === "pairing";

/** The connection banner of a phone paired with several Macs: the ones that
 *  are down or coming back, by name — or by count, past one. Their agents stay
 *  in the list meanwhile, as they were last seen. Each client reconnects on
 *  its own; Retry dials again every one not already dialling, and is withheld
 *  while all of them are. */
export function HostsBanner() {
  const hosts = useApp((s) => s.hosts);
  const connections = useEveryHost((s) => s.connection);
  const retrying = useEveryHost((s) => s.retrying);
  const down = hosts.flatMap((host, i) =>
    connections[i] === "connected"
      ? []
      : [{ host, connection: connections[i], retrying: retrying[i] }],
  );
  if (down.length === 0) return null;
  const busy = down.every((d) => dialling(d.connection));
  const who = down.length === 1 ? down[0].host.name : `${down.length} hosts`;
  const text = busy
    ? `Reconnecting to ${who}…`
    : `${who} ${down.length === 1 ? "is" : "are"} offline`;
  const retry = () => {
    for (const d of down) {
      if (!dialling(d.connection)) {
        void hostStoreOf(d.host.hostKey).getState().reconnect().catch(ignore);
      }
    }
  };
  return (
    <Notice
      tone={down.every((d) => dialling(d.connection) || d.retrying) ? "warn" : "error"}
      icon={
        busy ? (
          <span className="working-dots">
            <i />
            <i />
            <i />
          </span>
        ) : undefined
      }
      className="conn"
      action={busy ? undefined : { label: "Retry", onClick: retry }}
    >
      {text}
    </Notice>
  );
}
