import { useEffect, useMemo, useRef } from "react";
import type { AgentRecord } from "../../api";
import { useAppStore } from "../../store";
import { applyPolicy, getAdapter } from "../../adapters";
import { providerLabel } from "../../data/providers";
import { Composer } from "../Composer";
import { MessageItem } from "./messages/MessageItem";
import { pairToolItems, type ViewItem } from "./messages/pair";

/** Stable React key for a rendered row. Tool pairs/results key off their tool
 *  id so a row's expand state stays anchored to the tool rather than to a list
 *  position. Messages and notices have no id and the log is append-only, so
 *  their array index is a stable key — and keying them by text would remount on
 *  every streaming token. */
function rowKey(item: ViewItem, index: number): string {
  switch (item.kind) {
    case "tool_pair":
      return item.call.id ? `tp:${item.call.id}` : `i:${index}`;
    // No `tool_call` case: pairToolItems wraps every tool_call into a
    // tool_pair, so only orphan tool_results reach here standalone.
    case "tool_result":
      return item.tool_use_id ? `tr:${item.tool_use_id}` : `i:${index}`;
    default:
      return `i:${index}`;
  }
}

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
    if (transcriptLoading) return;
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
              <div>
                {providerLabel(agent.provider)}'s session file is not on disk
                for this agent.
              </div>
            </div>
          ) : (
            items.map((item, i) => (
              <MessageItem key={rowKey(item, i)} item={item} />
            ))
          )}
          {busy && (
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
          onSend={({ text, thinking, attachments }) => send(agent.id, text, attachments, thinking)}
          onStop={() => stop(agent.id)}
        />
      </div>
    </div>
  );
}
