// What a chat shows while it has no log to draw. A chat is created by its first
// message, so "no messages" is never the answer: the history is either still
// on its way from the host, or the read failed, or the host had none to give —
// and each of those says so (see `LogLoad`).

import { Icon } from "@desktop/components/Icon";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";
import type { LogLoad } from "../../store/transcript";
import { Dots } from "./Transcript";

/** The shape of a conversation — a prompt, a reply, a tool card, another
 *  turn — shimmering where the real one will land. */
export function ChatSkeleton() {
  return (
    <div className="log-skel" role="status" aria-label="Loading messages">
      <div className="sk sk-user" style={{ width: "64%" }} />
      <div className="sk-text">
        <div className="sk" style={{ width: "94%" }} />
        <div className="sk" style={{ width: "86%" }} />
        <div className="sk" style={{ width: "52%" }} />
      </div>
      <div className="sk sk-tool" />
      <div className="sk sk-user" style={{ width: "42%" }} />
      <div className="sk-text">
        <div className="sk" style={{ width: "90%" }} />
        <div className="sk" style={{ width: "68%" }} />
      </div>
      <div className="log-skel-cap">
        <Dots />
        Loading messages
      </div>
    </div>
  );
}

/** The read failed (`error`), or it succeeded with no history in it (`null`).
 *  Either way the conversation exists on the host; this offers to read again
 *  rather than inviting a "first" message into a chat that already has some. */
export function LogUnavailable({ agentId, error }: { agentId: string; error: string | null }) {
  const loadAgent = useStore((s) => s.loadAgent);
  const connected = useStore((s) => s.connection === "connected");
  const host = useStore((s) => s.hostInfo?.name) ?? "your Mac";
  return (
    <div className="blank log-fail fade">
      <div className="glyph danger">
        <Icon name="alert" size={28} strokeWidth={1.5} />
      </div>
      <h2>{error ? "Couldn’t load messages" : "Messages unavailable"}</h2>
      <div className="body">
        <p>
          {!connected
            ? `${host} is offline. Messages load once it reconnects.`
            : error
              ? `${host} didn’t return this conversation.`
              : `${host} has no stored history for this conversation yet.`}
        </p>
        {error && <p className="why">{error}</p>}
      </div>
      <div className="acts">
        <button
          type="button"
          className="btn"
          disabled={!connected}
          onClick={() => void loadAgent(agentId).catch(ignore)}
        >
          <Icon name="refresh" size={15} />
          Try again
        </button>
      </div>
    </div>
  );
}

/** The placeholder for a log that has nothing to show, by how its read went.
 *  `busy` covers the one honest gap: a turn still running before the host has
 *  stored any of it, where the working indicator says what there is to say. */
export function LogPlaceholder({
  agentId,
  load,
  busy,
}: {
  agentId: string;
  load: LogLoad | undefined;
  busy: boolean;
}) {
  if (!load || load.status === "loading") return <ChatSkeleton />;
  if (load.status === "error") return <LogUnavailable agentId={agentId} error={load.error} />;
  return busy ? null : <LogUnavailable agentId={agentId} error={null} />;
}
