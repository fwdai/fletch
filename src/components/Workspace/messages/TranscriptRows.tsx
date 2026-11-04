// The rows of one agent's transcript — nothing else. No scroll container, no
// bottom-pinning, no navigator: those belong to whoever owns the scroll.
//
// Extracted from TranscriptList so a surface that stacks several agents' logs in
// one scroll container (the run monitor's thread) renders identical rows from the
// identical derivation instead of a second copy of this mapping.
//
// The rows are the row model's (rows.ts): runs of steps gathered behind one
// line, and what the agent says and the sub-agents it starts on the page. This
// is the desktop's shell around the shared rows: it draws messages with
// MessageItem and provides the app's background tasks, reveal requests and
// markdown (TranscriptHost).
import { Fragment, useCallback, useMemo, useRef } from "react";
import type { AgentRecord } from "@/api";
import { Markdown } from "@/components/Markdown";
import { Loader } from "@/components/ui/Loader";
import { providerLabel } from "@/data/providers";
import { taskForToolUse, useAppStore } from "@/store";
import { TurnFooter } from "../RunTimer";
import { RowEnv, type RowEnvValue, TranscriptHost, type TranscriptHostValue } from "./host";
import { MessageItem } from "./MessageItem";
import { RowView, rowViewKey } from "./RowView";
import { buildRows, footerFor, newRowCache } from "./rows";
import type { Transcript } from "./useTranscript";

function HostMarkdown({ text }: { text: string }) {
  return <Markdown>{text}</Markdown>;
}

export function TranscriptRows({
  agent,
  transcript,
  liveBusy,
  pending,
}: {
  agent: AgentRecord;
  transcript: Transcript;
  /** The agent is mid-turn: rows in the open turn may show a live spinner. */
  liveBusy: boolean;
  /** Quiet inline anchor for a just-sent first prompt with nothing back yet. */
  pending?: boolean;
}) {
  const { items, turnIds, turnFooters, openTurnStart, transcriptLoading } = transcript;

  // Runs keep their identity across streaming deltas (see RowCache).
  const cache = useRef(newRowCache());
  const { rows, tailRunKey } = useMemo(() => buildRows(items, cache.current), [items]);

  const tasks = useAppStore((s) => s.backgroundTasks[agent.id]);
  const focus = useAppStore((s) => s.chatFocus);
  const clearFocus = useAppStore((s) => s.clearChatFocus);
  const host = useMemo<TranscriptHostValue>(
    () => ({
      taskFor: (toolUseId) => taskForToolUse(tasks, toolUseId),
      focus,
      clearFocus,
      Markdown: HostMarkdown,
    }),
    [tasks, focus, clearFocus],
  );

  const renderItem = useCallback(
    (index: number, busy: boolean) => (
      <MessageItem
        item={items[index]}
        provider={agent.provider}
        agentId={agent.id}
        busy={busy}
        turnId={turnIds[index]}
      />
    ),
    [items, turnIds, agent.provider, agent.id],
  );
  const env = useMemo<RowEnvValue>(
    () => ({
      agentId: agent.id,
      provider: agent.provider,
      items,
      liveBusy,
      openTurnStart,
      tailRunKey,
      renderItem,
    }),
    [agent.id, agent.provider, items, liveBusy, openTurnStart, tailRunKey, renderItem],
  );

  if (transcriptLoading && items.length === 0) {
    return (
      <div className="writing flex-center">
        <Loader variant="accent" />
        <span>Loading transcript…</span>
      </div>
    );
  }

  if (items.length === 0 && transcript.hasPriorConversation && !liveBusy) {
    return (
      <div className="empty-msg" style={{ margin: "40px auto", maxWidth: 360 }}>
        <div className="et">No transcript available</div>
        <div>{providerLabel(agent.provider)}'s session file is not on disk for this agent.</div>
      </div>
    );
  }

  return (
    <TranscriptHost.Provider value={host}>
      <RowEnv.Provider value={env}>
        {rows.map((row) => {
          const footer = footerFor(row, turnFooters, liveBusy, openTurnStart);
          return (
            <Fragment key={rowViewKey(row, items)}>
              <RowView row={row} />
              {footer != null && <TurnFooter {...footer} agentId={agent.id} />}
            </Fragment>
          );
        })}
        {pending && (
          <div className="chat-pending" aria-hidden="true">
            <Loader variant="muted" size="md" />
          </div>
        )}
      </RowEnv.Provider>
    </TranscriptHost.Provider>
  );
}
