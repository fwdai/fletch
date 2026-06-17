import { useEffect, useMemo, useRef } from "react";
import { api, type AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { applyPolicy, getAdapter } from "../../adapters";
import { providerLabel } from "../../data/providers";
import { Composer } from "../Composer";
import { MessageItem } from "./messages/MessageItem";
import { pairToolItems, rowKey } from "./messages/pair";
import { isUserInputTool } from "./messages/UserInput/parse";

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
  const busyLabel = useAppStore((s) => s.managedBusyLabel[agent.id]);
  const switchInFlight = useAppStore(
    (s) => s.switchInFlight[agent.id] ?? false,
  );
  const send = useAppStore((s) => s.sendUserMessage);
  const stop = useAppStore((s) => s.stop);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);

  const scrollRef = useRef<HTMLDivElement>(null);
  // Whether the chat is "pinned" to the bottom. While true we follow new
  // messages; once the user scrolls up we stop auto-scrolling and leave their
  // position alone until they scroll back down to the bottom.
  const pinnedToBottom = useRef(true);
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

  // Re-pin to the bottom whenever we switch to a different agent, so each
  // conversation opens scrolled to its latest message.
  useEffect(() => {
    pinnedToBottom.current = true;
  }, [agent.id]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    // Allow a small slop so the user counts as "at the bottom" even a few
    // pixels short — sub-pixel rounding and the trailing typing indicator
    // otherwise make exact equality flaky.
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    pinnedToBottom.current = distanceFromBottom <= 40;
  };

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (transcriptLoading) return;
    if (!pinnedToBottom.current) return;
    el.scrollTop = el.scrollHeight;
  }, [log, transcriptLoading]);

  const items = useMemo(() => {
    const adapter = getAdapter(agent.provider);
    const visible = applyPolicy(log ?? [], adapter.policy);
    return pairToolItems(visible);
  }, [log, agent.provider]);

  // The model the agent actually used on its most recent turn (Claude, pi,
  // Codex, OpenCode report it in their transcripts). Undefined for Cursor /
  // Antigravity, or before the first turn — the composer then shows just the
  // provider.
  const activeModel = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      const it = items[i];
      if (it.kind === "agent_message" && it.model) return it.model;
    }
    return undefined;
  }, [items]);
  // When the last row is an unanswered user-input widget, the agent is paused
  // on that tool waiting for the user — not working — so suppress the
  // "is thinking" spinner (the widget itself signals it's the user's turn).
  const awaitingInput = useMemo(() => {
    const last = items[items.length - 1];
    return Boolean(
      last &&
        last.kind === "tool_pair" &&
        isUserInputTool(last.call.name) &&
        !last.result,
    );
  }, [items]);

  const canSend =
    !transcriptLoading &&
    !switchInFlight &&
    !busy &&
    (agent.status === "running" || agent.status === "idle");

  return (
    <div className="chat">
      <div className="chat-scroll" ref={scrollRef} onScroll={handleScroll}>
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
              <div>
                {providerLabel(agent.provider)}'s session file is not on disk
                for this agent.
              </div>
            </div>
          ) : (
            items.map((item, i) => (
              <MessageItem
                key={rowKey(item, i)}
                item={item}
                provider={agent.provider}
                agentId={agent.id}
              />
            ))
          )}
          {busy && !awaitingInput && (
            <div className="writing">
              <span className="dots">
                <i /><i /><i />
              </span>
              <span>
                {busyLabel ?? `${providerLabel(agent.provider)} is thinking`}
              </span>
            </div>
          )}
        </div>
      </div>
      <div className="composer-wrap">
        <Composer
          existingSession
          activeModel={activeModel}
          defaultProvider={agent.provider}
          initialThinking={agent.effort ?? undefined}
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
          mentionSource={() =>
            api.listWorktreeTree(agent.id).then((files) => files.map((f) => f.path))
          }
          listDir={api.listDir}
          listPrs={() => api.listPrs(agent.id)}
          onSend={({ text, thinking, attachments }) => {
            // Sending is an explicit action: re-pin so the user follows their
            // own new message even if they'd scrolled up to read history.
            pinnedToBottom.current = true;
            send(agent.id, text, attachments, thinking);
          }}
          onStop={() => stop(agent.id)}
        />
      </div>
    </div>
  );
}
