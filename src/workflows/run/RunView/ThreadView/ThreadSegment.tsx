// ThreadView/ThreadSegment — one step agent's stretch of the thread: its
// hand-off marker plus its transcript rows, rendered into the *thread's* scroll
// container rather than one of its own.
//
// The rows and the derivation behind them are the chat's, not a copy: the
// segment calls the same useTranscript (lazy history load, display policy, tool
// pairing, turn footers) and renders the same TranscriptRows. Only the live
// segment gets the live behaviors — a settled step must never show a spinner.

import type { AgentRecord } from "../../../../api";
import { TranscriptRows } from "../../../../components/Workspace/messages/TranscriptRows";
import { isTurnPending } from "../../../../components/Workspace/messages/turnPending";
import { useTranscript } from "../../../../components/Workspace/messages/useTranscript";
import { useLiveBusy } from "../../../../components/Workspace/useLiveBusy";
import type { ResolvedAgent } from "../../../shared";
import { HandoffMarker } from "./HandoffMarker";
import type { Segment } from "./segments";

export function ThreadSegment({
  segment,
  stepCount,
  resolved,
}: {
  segment: Segment;
  stepCount: number;
  resolved: ResolvedAgent | null;
}) {
  return (
    <div className="wf-thread-seg">
      <HandoffMarker segment={segment} stepCount={stepCount} agent={resolved} />
      {segment.agent ? (
        <SegmentBody agent={segment.agent} live={segment.live} />
      ) : (
        <div className="wf-thread-gap">
          {segment.exec.agent_id
            ? "This step's chat is no longer loaded."
            : "This step hasn't started its agent yet."}
        </div>
      )}
    </div>
  );
}

/** Split out so the transcript hooks only ever run for a segment that has an
 *  agent record to hook onto. */
function SegmentBody({ agent, live }: { agent: AgentRecord; live: boolean }) {
  const transcript = useTranscript(agent);
  const busy = useLiveBusy(agent.id, transcript.awaitingInput);
  // An archived agent can still read `busy` from a stale store entry; only the
  // attempt the run is actually working may render as live.
  const liveBusy = live && busy;
  const pending = liveBusy && isTurnPending(transcript.items) && transcript.turns.length <= 1;

  return (
    <TranscriptRows agent={agent} transcript={transcript} liveBusy={liveBusy} pending={pending} />
  );
}
