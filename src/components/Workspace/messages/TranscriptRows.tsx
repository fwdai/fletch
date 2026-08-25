// The rows of one agent's transcript — nothing else. No scroll container, no
// bottom-pinning, no navigator: those belong to whoever owns the scroll.
//
// Extracted from TranscriptList so a surface that stacks several agents' logs in
// one scroll container (the run monitor's thread) renders identical rows from the
// identical derivation instead of a second copy of this mapping.
import { Fragment } from "react";
import type { AgentRecord } from "@/api";
import { Loader } from "@/components/ui/Loader";
import { providerLabel } from "@/data/providers";
import { TurnFooter } from "../RunTimer";
import { MessageItem } from "./MessageItem";
import { rowKey } from "./pair";
import type { Transcript } from "./useTranscript";

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
    <>
      {items.map((item, i) => {
        const footer = turnFooters[i];
        return (
          <Fragment key={rowKey(item, i)}>
            <MessageItem
              item={item}
              provider={agent.provider}
              agentId={agent.id}
              busy={liveBusy && i >= openTurnStart}
              turnId={turnIds[i]}
            />
            {footer != null && <TurnFooter {...footer} agentId={agent.id} />}
          </Fragment>
        );
      })}
      {pending && (
        <div className="chat-pending" aria-hidden="true">
          <Loader variant="muted" size="md" />
        </div>
      )}
    </>
  );
}
