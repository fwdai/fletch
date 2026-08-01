import { useRef } from "react";
import type { AgentRecord } from "@/api";
import { ChatWorkingStatus } from "@/components/Workspace/ChatWorkingStatus";
import { TranscriptList } from "@/components/Workspace/messages/TranscriptList";
import { isTurnPending } from "@/components/Workspace/messages/turnPending";
import { useTranscript } from "@/components/Workspace/messages/useTranscript";
import { useLiveBusy } from "@/components/Workspace/useLiveBusy";
import { providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";
import { Composer } from "./Composer";

/** Statuses a message can be sent into. `spawning` is included deliberately:
 *  the chat opens the instant its record exists and provisions in the
 *  background, so making the user wait for a clone before they can type would
 *  undo the point of that. The backend queues the turn until the process is up. */
const SENDABLE = new Set(["running", "idle", "spawning"]);

/** One PM chat, rendered as the real transcript it is.
 *
 *  Deliberately not `<ChatView>`: this column is half the width of the
 *  workspace pane and the chat is advisory, so the agent/model picker, issue
 *  picker, attachments, find-in-conversation and turn navigator would all be
 *  noise here. What it does share is everything that decides *what the rows
 *  mean* — `useTranscript`, the provider adapters, `TranscriptList` — so a
 *  tool call, a thinking block or a question card looks and behaves exactly as
 *  it does in the main chat, and neither surface can drift from the other. */
export function ChatPane({ agent }: { agent: AgentRecord }) {
  const send = useAppStore((s) => s.sendUserMessage);
  const turnStartedAt = useAppStore((s) => s.turnStartedAt[agent.id]);
  const busyLabel = useAppStore((s) => s.managedBusyLabel[agent.id]);
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );

  const transcript = useTranscript(agent);
  const { items, turns, awaitingInput, openTurnStartedAt, transcriptLoading } = transcript;
  const liveBusy = useLiveBusy(agent.id, awaitingInput);

  // Owned here so sending re-pins the log to the bottom, as in the main chat.
  const pinnedToBottom = useRef(true);

  // Quiet inline anchor for a just-sent first prompt with nothing back yet;
  // later turns have content above and the status strip below to carry it.
  const pending = liveBusy && isTurnPending(items) && turns.length <= 1;
  // The backend's own turn-start timestamp, falling back to the open turn's
  // persisted start after a reload — the same anchor the main chat's timer uses.
  const liveStartedAt = liveBusy ? (turnStartedAt ?? openTurnStartedAt) : undefined;

  const canSend = !transcriptLoading && SENDABLE.has(agent.status);
  const placeholder = transcriptLoading
    ? "Loading the conversation…"
    : agent.status === "spawning"
      ? "Setting up the workspace — say what you want to shape…"
      : canSend
        ? "Describe an outcome, a complaint, a half-formed idea…"
        : agent.status === "error"
          ? "This chat hit an error — start a new one."
          : "This chat is stopped.";

  return (
    <div className="rm-chat">
      <TranscriptList
        agent={agent}
        transcript={transcript}
        liveBusy={liveBusy}
        pending={pending}
        pinRef={pinnedToBottom}
        hideNav
      />
      <Composer
        disabled={!canSend}
        placeholder={placeholder}
        status={
          <ChatWorkingStatus
            visible={liveBusy}
            label={busyLabel ?? `${customAgent?.name ?? providerLabel(agent.provider)} is working`}
            startedAt={liveStartedAt}
          />
        }
        onSend={(text) => {
          pinnedToBottom.current = true;
          void send(agent.id, text);
        }}
      />
    </div>
  );
}
