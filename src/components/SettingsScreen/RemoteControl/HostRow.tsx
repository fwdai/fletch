import { HostGateNote } from "@/components/HostGateNote";
import { HostProvidersNote } from "@/components/HostProvidersNote";
import { Button } from "@/components/ui/Button";
import type { SavedHost } from "@/storage/remoteHosts";
import { useAppStore } from "@/store";
import { hostVersionLabel } from "@/store/capabilities";
import type { ConnectionStatus } from "@/store/environments";

/** What the dot's colour is saying, in words. */
const STATE_LABELS: Record<ConnectionStatus, string> = {
  connected: "connected",
  connecting: "connecting…",
  disconnected: "not connected",
  error: "not connected",
};

/** One paired host: where it is, whether this Mac is talking to it right now,
 *  and the last reason it isn't. "Forget" drops the connection and the record;
 *  the host keeps its own device entry until it is revoked there, so pairing
 *  again needs a fresh link either way. */
export function HostRow({
  host,
  disabled,
  onForget,
}: {
  host: SavedHost;
  disabled?: boolean;
  onForget: () => void;
}) {
  // The live side of the record: the lifecycle publishes one entry per saved
  // host under its key, so this is the same host seen from the store.
  const entry = useAppStore((s) => s.environments[host.hostKey]);
  // No entry means no client: a saved record whose re-pairing was refused, or
  // one added by another window. Either way nothing is talking to it.
  const connection = entry?.connection ?? "disconnected";
  // What the host said about itself at the last handshake: its version, and
  // (through the note below) which of this app's features it cannot offer.
  const version = entry ? hostVersionLabel(entry) : null;

  return (
    <div className="set-row flex-center align-start">
      <div className="set-row-l">
        <div className="set-row-t text-base flex-center">
          <i className="set-dev-dot" data-state={connection} />
          {entry?.name || host.name}
        </div>
        <div className="set-row-s text-sm">
          {host.addr} · {STATE_LABELS[connection]}
          {host.relay ? " · relay configured" : ""}
          {version ? ` · ${version}` : ""}
        </div>
        {entry?.error && <div className="set-row-s text-sm">{entry.error}</div>}
        {entry && <HostGateNote env={entry} className="set-row-s text-sm" />}
        {entry && <HostProvidersNote env={entry} className="set-row-s text-sm" />}
      </div>
      <div className="set-row-c flex-center">
        <Button variant="outline" size="sm" danger disabled={disabled} onClick={onForget}>
          Forget
        </Button>
      </div>
    </div>
  );
}
