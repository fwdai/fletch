import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

const LABEL: Record<string, string> = {
  connecting: "Connecting to the host…",
  pairing: "Pairing…",
  disconnected: "Not connected to the host",
};

/** Shown whenever the link is anything other than connected. The client
 *  reconnects on its own; the button is for the impatient. */
export function ConnectionBanner() {
  const connection = useStore((s) => s.connection);
  const error = useStore((s) => s.connectionError);
  const reconnect = useStore((s) => s.reconnect);
  if (connection === "connected") return null;
  const isError = connection === "error";
  return (
    <div className={`conn${isError ? " err" : ""}`}>
      <Icon name={isError ? "alert" : "refresh"} size={14} />
      <span className="grow">{isError ? (error ?? "Connection lost") : LABEL[connection]}</span>
      <button type="button" onClick={() => void reconnect().catch(ignore)}>
        Retry
      </button>
    </div>
  );
}
