// The rows of one agent's transcript — nothing else. No scroll container, no
// bottom-pinning, no navigator: those belong to whoever owns the scroll.
//
// Extracted from TranscriptList so a surface that stacks several agents' logs in
// one scroll container (the run monitor's thread) renders identical rows from the
// identical derivation instead of a second copy of this mapping.
//
// The rows are the row model's (rows.ts): runs of steps gathered behind one
// line, and every settled turn but the latest folded down to its prompt, one
// "Worked for …" line and its answer.
import { Fragment, useMemo, useRef } from "react";
import type { AgentRecord } from "@/api";
import { Loader } from "@/components/ui/Loader";
import { providerLabel } from "@/data/providers";
import { TurnFooter } from "../RunTimer";
import { RowEnv, type RowEnvValue, RowView, rowViewKey } from "./RowView";
import { buildRows, lastIndex, type RowCache } from "./rows";
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

  // Runs and folds keep their identity across streaming deltas (see RowCache).
  const cache = useRef<RowCache>(new Map());
  const { rows, quietFooters } = useMemo(() => buildRows(items, cache.current), [items]);

  // The run the transcript ends on — a mid-turn follow-up bubble below it
  // aside — is the one the agent is on while the turn is live.
  const tailRunKey = useMemo(() => {
    for (let r = rows.length - 1; r >= 0; r -= 1) {
      const row = rows[r];
      if (row.kind === "item" && items[row.index].kind === "queued_message") continue;
      return row.kind === "steps" ? row.run.key : null;
    }
    return null;
  }, [rows, items]);

  const env = useMemo<RowEnvValue>(
    () => ({ agent, items, turnIds, liveBusy, openTurnStart, tailRunKey }),
    [agent, items, turnIds, liveBusy, openTurnStart, tailRunKey],
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
    <RowEnv.Provider value={env}>
      {rows.map((row) => {
        const end = lastIndex(row);
        const footer = turnFooters[end];
        return (
          <Fragment key={rowViewKey(row, items)}>
            <RowView row={row} />
            {footer != null && (
              <TurnFooter {...footer} agentId={agent.id} quiet={quietFooters.has(end)} />
            )}
          </Fragment>
        );
      })}
      {pending && (
        <div className="chat-pending" aria-hidden="true">
          <Loader variant="muted" size="md" />
        </div>
      )}
    </RowEnv.Provider>
  );
}
