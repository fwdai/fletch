// ThreadView/ThreadComposer — one composer for the whole thread, routed by what
// the run can accept right now:
//
//   live      → the step agent that is working: the normal chat composer, same
//               send machinery as any other agent (ChatComposer).
//   otherwise → disabled with a short reason. Never a live-looking box that
//               silently drops what the user types.

import type { AgentRecord, WfRun } from "../../../../api";
import { ChatComposer } from "../../../../components/Workspace/ChatComposer";
import { useTranscript } from "../../../../components/Workspace/messages/useTranscript";
import { useLiveBusy } from "../../../../components/Workspace/useLiveBusy";
import { useAppStore } from "../../../../store";
import { disabledHint } from "./composer";

export function ThreadComposer({
  run,
  live,
  onSend,
}: {
  run: WfRun;
  /** The step agent currently working, when there is one. */
  live: AgentRecord | undefined;
  /** Re-pin the thread to the bottom — a send should scroll like a chat send. */
  onSend: () => void;
}) {
  if (live) return <LiveComposer agent={live} onSend={onSend} />;

  return (
    <div className="composer-wrap">
      <div className="wf-thread-hint">{disabledHint(run)}</div>
    </div>
  );
}

/** The live step agent's composer. Mounted only while such an agent exists, so
 *  its transcript hooks can be unconditional; the derivation is the same
 *  memoized pass the agent's own segment runs, and the composer sits outside the
 *  scroll container, so it reads it here rather than threading it through. */
function LiveComposer({ agent, onSend }: { agent: AgentRecord; onSend: () => void }) {
  const turnStartedAt = useAppStore((s) => s.turnStartedAt[agent.id]);
  const transcript = useTranscript(agent);
  const liveBusy = useLiveBusy(agent, transcript.awaitingInput);
  const liveStartedAt = liveBusy ? (turnStartedAt ?? transcript.openTurnStartedAt) : undefined;

  return (
    <ChatComposer
      agent={agent}
      activeModel={transcript.activeModel}
      liveBusy={liveBusy}
      liveStartedAt={liveStartedAt}
      onSend={onSend}
    />
  );
}
