import { useEffect, useMemo, useRef } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { applyPolicy, getAdapter } from "../../adapters";
import { Composer } from "../Composer";
import { MessageItem } from "./messages/MessageItem";
import { pairToolItems } from "./messages/pair";

/** Custom-view body: scrolling chat log + composer at the bottom.
 *  The composer here dispatches the user's message via the store; it
 *  doesn't care about provider routing yet. */
export function ChatView({ agent }: { agent: AgentRecord }) {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const transcriptLoading = useAppStore(
    (s) => s.transcriptLoading[agent.id] ?? false,
  );
  const transcriptLoaded = useAppStore(
    (s) => s.transcriptLoaded[agent.id] ?? false,
  );
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const switchInFlight = useAppStore(
    (s) => s.switchInFlight[agent.id] ?? false,
  );
  const send = useAppStore((s) => s.sendUserMessage);
  const stop = useAppStore((s) => s.stop);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);

  const scrollRef = useRef<HTMLDivElement>(null);
  const wasTranscriptLoading = useRef(false);
  const hasSession = Boolean(agent.session_id);
  const hasPriorConversation = agent.task.trim().length > 0;

  useEffect(() => {
    if (
      !hasSession ||
      transcriptLoaded ||
      transcriptLoading ||
      switchInFlight
    ) {
      return;
    }
    if (log !== undefined || !hasPriorConversation) {
      return;
    }
    void loadHistoryTranscript(agent.id);
  }, [
    agent.id,
    hasSession,
    hasPriorConversation,
    loadHistoryTranscript,
    log,
    switchInFlight,
    transcriptLoaded,
    transcriptLoading,
  ]);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (wasTranscriptLoading.current && !transcriptLoading) {
      el.scrollTop = 0;
      wasTranscriptLoading.current = false;
      return;
    }
    wasTranscriptLoading.current = transcriptLoading;
    if (transcriptLoading) return;
    el.scrollTop = el.scrollHeight;
  }, [log, transcriptLoading]);

  const items = useMemo(() => {
    const adapter = getAdapter(agent.provider);
    const visible = applyPolicy(log ?? [], adapter.policy);
    return pairToolItems(visible);
  }, [log, agent.provider]);
  const canSend =
    !transcriptLoading &&
    !switchInFlight &&
    !busy &&
    (agent.status === "running" || agent.status === "idle");

  return (
    <div className="chat">
      <div className="chat-scroll" ref={scrollRef}>
        <div className="chat-inner fade-in" key={agent.id}>
          {transcriptLoading && items.length === 0 ? (
            <div className="writing">
              <span className="dots">
                <i /><i /><i />
              </span>
              <span>Loading transcript…</span>
            </div>
          ) : items.length === 0 && hasPriorConversation && !busy ? (
            <div
              className="empty-msg"
              style={{ margin: "40px auto", maxWidth: 360 }}
            >
              <div className="et">No transcript available</div>
              <div>Claude's session file is not on disk for this agent.</div>
            </div>
          ) : (
            items.map((item, i) => <MessageItem key={i} item={item} />)
          )}
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
              ? undefined
              : transcriptLoading
                ? "Loading transcript…"
                : switchInFlight
                  ? "Switching view…"
                : busy
                  ? "Waiting…"
                  : "Agent is not ready"
          }
          stopping={busy}
          onSend={({ text }) => send(agent.id, text)}
          onStop={() => stop(agent.id)}
        />
      </div>
    </div>
  );
}
