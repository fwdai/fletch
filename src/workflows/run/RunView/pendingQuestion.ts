// RunView/pendingQuestion.ts — select the human-answerable question for a
// paused(question) run. Mirrors the backend's `has_unanswered_ask` selection
// (comms.rs): the answerable ask is the queued `ask` *from the exec that
// paused* — it is NOT keyed on the message's recipient. Escalations and
// engine-authored asks carry the orchestrator's exec in `from_step_exec_id`
// (and the human-directed recipient may be null or a step exec), so keying on
// `to_step_exec_id` misses them; keying on `from_step_exec_id` does not.

import type { WfMessage } from "../../../api";

/** The queued `ask` the human must answer. `pausedExec` is the `step_exec_id`
 *  of the `run_paused` event. When it's known we match that exec precisely (so
 *  a child's ask still queued to an orchestrator is never mistaken for the
 *  human's); when it's not yet loaded we best-effort the first queued ask so
 *  the banner is never wedged on "Loading the question…". */
export function selectPendingQuestion(
  messages: WfMessage[],
  pausedExec: string | null,
): WfMessage | undefined {
  const queued = messages.filter((m) => m.kind === "ask" && m.status === "queued");
  if (pausedExec) return queued.find((m) => m.from_step_exec_id === pausedExec);
  return queued[0];
}
