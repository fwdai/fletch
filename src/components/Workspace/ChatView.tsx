import { useEffect, useRef } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { Composer } from "../Composer";
import { MessageItem } from "./messages/MessageItem";

/** Custom-view body: scrolling chat log + composer at the bottom.
 *  The composer here dispatches the user's message via the store; it
 *  doesn't care about provider routing yet. */
export function ChatView({ agent }: { agent: AgentRecord }) {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const send = useAppStore((s) => s.sendUserMessage);

  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [log]);

  const canSend =
    !busy && (agent.status === "running" || agent.status === "idle");

  return (
    <div className="chat">
      <div className="chat-scroll" ref={scrollRef}>
        <div className="chat-inner fade-in" key={agent.id}>
          {(log ?? []).map((item, i) => (
            <MessageItem key={i} item={item} />
          ))}
          {busy && (
            <div className="writing">
              <span className="dots">
                <i /><i /><i />
              </span>
              <span>{agent.name} is thinking</span>
            </div>
          )}
        </div>
      </div>
      <div className="composer-wrap">
        <Composer
          disabled={!canSend}
          placeholder={
            canSend
              ? "Send a follow-up — ⌘↵ to send"
              : busy ? "Waiting…" : "Agent is not ready"
          }
          onSend={({ text }) => send(agent.id, text)}
        />
      </div>
    </div>
  );
}
