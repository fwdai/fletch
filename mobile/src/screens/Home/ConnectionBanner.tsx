import { Notice } from "../../components/ui/Notice";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

/** The slim status line over a cached project list while the link to the Mac
 *  is down or coming back. Persistent by nature — it is the connection state,
 *  not a message about it — so there is no dismiss; it goes when the link is
 *  up. The client reconnects on its own; Retry is for the impatient, and is
 *  withheld while an attempt is already running.
 *
 *  Home shows it only over a list. With nothing cached the whole body is the
 *  connection state instead (`HostState`). */
export function ConnectionBanner() {
  const connection = useStore((s) => s.connection);
  const error = useStore((s) => s.connectionError);
  const retrying = useStore((s) => s.retrying);
  const reconnect = useStore((s) => s.reconnect);
  if (connection === "connected") return null;
  const busy = connection === "connecting" || connection === "pairing";
  const text = busy
    ? "Reconnecting to your Mac…"
    : connection === "error"
      ? `${error ?? "Connection lost"}${retrying ? " · reconnecting" : ""}`
      : "Not connected to your Mac";
  return (
    <Notice
      tone={busy || retrying ? "warn" : "error"}
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
      action={busy ? undefined : { label: "Retry", onClick: () => void reconnect().catch(ignore) }}
    >
      {text}
    </Notice>
  );
}
