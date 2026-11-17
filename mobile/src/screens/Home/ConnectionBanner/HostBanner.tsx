import { Notice } from "../../../components/ui/Notice";
import { isProblem } from "../../../lib/connection";
import { ignore } from "../../../lib/ignore";
import { useHostStore } from "../../../store";

/** The connection banner of a phone paired with one Mac: the link to it down
 *  or coming back. The client reconnects on its own; Retry is for the
 *  impatient, and is withheld while an attempt is already running. */
export function HostBanner() {
  const connection = useHostStore((s) => s.connection);
  const error = useHostStore((s) => s.connectionError);
  const retrying = useHostStore((s) => s.retrying);
  const reconnect = useHostStore((s) => s.reconnect);
  if (connection === "connected") return null;
  const busy = connection === "connecting" || connection === "pairing";
  const text = busy
    ? "Reconnecting to your Mac…"
    : connection === "error"
      ? `${error ?? "Connection lost"}${retrying ? " · reconnecting" : ""}`
      : "Not connected to your Mac";
  return (
    <Notice
      tone={isProblem(connection, retrying) ? "error" : "warn"}
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
