// ThreadView/ThreadComposer — one composer for the whole thread, routed by what
// the run can accept right now:
//
//   live      → the step agent that is working: the normal chat composer, same
//               send machinery as any other agent (ChatComposer).
//   question  → the run is paused on an ask: the existing wf_answer form, docked
//               where the user is already typing.
//   otherwise → disabled with a short reason. Never a live-looking box that
//               silently drops what the user types.

import type { AgentRecord, WfMessage, WfRun } from "../../../../api";
import { ChatComposer } from "../../../../components/Workspace/ChatComposer";
import { useTranscript } from "../../../../components/Workspace/messages/useTranscript";
import { useLiveBusy } from "../../../../components/Workspace/useLiveBusy";
import { useAppStore } from "../../../../store";
import { AnswerForm } from "../PausedBanner/AnswerForm";
import { composerRoute, disabledHint } from "./composer";

export function ThreadComposer({
  run,
  question,
  live,
  onSend,
}: {
  run: WfRun;
  /** The pending human `ask` when the run is paused on a question. */
  question?: WfMessage;
  /** The step agent currently working, when there is one. */
  live: AgentRecord | undefined;
  /** Re-pin the thread to the bottom — a send should scroll like a chat send. */
  onSend: () => void;
}) {
  const setLastError = useAppStore((s) => s.setLastError);
  const route = composerRoute(run, live);

  if (route === "question") {
    return (
      <div className="composer-wrap">
        <div className="wf-thread-answer">
          <AnswerForm run={run} question={question} onError={setLastError} />
        </div>
      </div>
    );
  }

  if (route === "live" && live) {
    return <LiveComposer agent={live} onSend={onSend} />;
  }

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
  const liveBusy = useLiveBusy(agent.id, transcript.awaitingInput);
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
