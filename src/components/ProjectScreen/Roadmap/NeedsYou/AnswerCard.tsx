// NeedsYou/AnswerCard.tsx — the inline answer for a run paused on a human
// question, so answering costs no trip to the run's tab.
//
// Reuses the monitor's widget wholesale (RunView/PausedBanner/AnswerForm): the
// same `wf_answer` call, the same option buttons, the same ⌘↵. What this adds is
// only the lookup — `wf_answer` addresses the pending `ask` *message*, which the
// run list doesn't carry, so the run's detail is loaded here and the answerable
// ask picked with the backend's own rule (pendingQuestion.ts).
//
// Nothing removes the card afterwards: answering resumes the run, `wf:run`
// updates the list, and the selector stops producing the card.

import { useMemo } from "react";
import { useAppStore } from "@/store";
import { AnswerForm } from "@/workflows/run/RunView/PausedBanner/AnswerForm";
import { selectPendingQuestion } from "@/workflows/run/RunView/pendingQuestion";
import { useRunDetail } from "@/workflows/run/RunView/useRunDetail";

export function AnswerCard({ runId }: { runId: string }) {
  const { detail, events } = useRunDetail(runId);
  const setLastError = useAppStore((s) => s.setLastError);
  const run = detail?.run ?? null;

  // The most recent `run_paused` event names the exec whose ask the human must
  // answer — the same pair RunView reads, because a child's ask queued to an
  // orchestrator is not the human's question.
  const pausedExec = useMemo(() => {
    for (let i = events.length - 1; i >= 0; i--) {
      if (events[i].type === "run_paused") return events[i].step_exec_id;
    }
    return null;
  }, [events]);

  const question = useMemo(
    () => selectPendingQuestion(detail?.messages ?? [], pausedExec),
    [detail?.messages, pausedExec],
  );

  // Never a dead end: the row above already says the run is waiting on an
  // answer, so say the form is on its way rather than rendering an empty gap.
  if (!run) return <div className="wf-answer-hint">Loading the question…</div>;

  return (
    <div className="rm-needs-answer">
      <AnswerForm run={run} question={question} onError={setLastError} />
    </div>
  );
}
