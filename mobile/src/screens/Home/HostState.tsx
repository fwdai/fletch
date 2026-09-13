import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../lib/ignore";
import { client, useStore } from "../../store";
import { STEP_LABELS, Working } from "../Pair/Progress";

/** Home's whole body while there is no workspace to show: the link to the Mac
 *  is down, coming up, or up with the snapshot still on its way. There is
 *  nothing to add and no agent to start until it is, so nothing here offers
 *  to — the screen says what the connection is doing and what, if anything,
 *  the user can do about it.
 *
 *  The one case that needs care is `error`: the client retries most failures
 *  on its own (a dropped socket, the Mac asleep), and those read as
 *  "reconnecting"; the few it cannot clear (this device unpaired, remote
 *  access switched off, a changed host key) are stated as they are and point
 *  at the Host sheet, where the fix lives. */
export function HostState() {
  const connection = useStore((s) => s.connection);
  const error = useStore((s) => s.connectionError);
  const retrying = useStore((s) => s.retrying);
  const step = useStore((s) => s.pairStep);
  const hostName = useStore((s) => s.hostInfo?.name);
  const reconnect = useStore((s) => s.reconnect);
  const openSheet = useStore((s) => s.openSheet);
  const host = hostName ?? client.target?.name ?? "your Mac";

  const tryNow = () => void reconnect().catch(ignore);
  const details = (
    <button type="button" className="lnk" onClick={() => openSheet("host")}>
      Host details
    </button>
  );

  let dot: string;
  let title: string;
  let body: React.ReactNode;
  let acts: React.ReactNode = null;

  if (connection === "connected") {
    dot = "running";
    title = "Loading your workspace";
    body = <Working>Fetching projects and agents from {host}…</Working>;
  } else if (connection === "connecting" || connection === "pairing") {
    dot = "waiting";
    title = connection === "pairing" ? `Pairing with ${host}` : `Connecting to ${host}`;
    body = <Working>{step ? STEP_LABELS[step] : "This usually takes a moment…"}</Working>;
  } else if (connection === "error" && retrying) {
    dot = "waiting";
    title = `Can't reach ${host}`;
    body = (
      <>
        <p>{error ?? "The connection was lost."}</p>
        <Working>Reconnecting automatically…</Working>
      </>
    );
    acts = (
      <>
        <button type="button" className="btn ghost" onClick={tryNow}>
          <Icon name="refresh" size={16} />
          Try now
        </button>
        {details}
      </>
    );
  } else if (connection === "error") {
    dot = "error";
    title = "Not connected";
    body = <p>{error ?? "The connection was lost."}</p>;
    acts = (
      <button type="button" className="btn ghost" onClick={() => openSheet("host")}>
        <Icon name="laptop" size={16} />
        Host details
      </button>
    );
  } else {
    dot = "stopped";
    title = "Not connected";
    body = <p>Reconnect to {host} to see your projects and agents.</p>;
    acts = (
      <>
        <button type="button" className="btn ghost" onClick={tryNow}>
          <Icon name="refresh" size={16} />
          Reconnect
        </button>
        {details}
      </>
    );
  }

  return (
    <div className="blank fade" key={connection}>
      <div className="glyph">
        <Icon name="laptop" size={30} strokeWidth={1.5} />
        <span className="badge">
          <span className={`dot ${dot}`} />
        </span>
      </div>
      <h2>{title}</h2>
      <div className="body">{body}</div>
      {acts && <div className="acts">{acts}</div>}
    </div>
  );
}
