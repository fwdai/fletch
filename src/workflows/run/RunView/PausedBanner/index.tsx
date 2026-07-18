// RunView/PausedBanner — the paused/failed banner (spec §14.2). Every state a
// run can rest in names its cause in plain language and offers its action:
// approve (S4), retry/resume (S4), answer a question (S10), and conflict
// resolution (S9). No dead buttons — each paused reason wires to its command.
//
// The four widgets live in sibling files: ReviewModal (approval), AnswerForm
// (question), ResumeBudgetForm (budget), and the specFor state-machine mapper
// (specs.ts). This file owns the banner shell and orchestrates them.

import { useEffect, useState } from "react";
import type { GateEvidence, WfMessage, WfRun } from "../../../../api";
import { Icon } from "../../../../components/Icon";
import { useAppStore } from "../../../../store";
import { AnswerForm } from "./AnswerForm";
import { ResumeBudgetForm } from "./ResumeBudgetForm";
import { ReviewModal } from "./ReviewModal";
import { type Action, specFor } from "./specs";

export function PausedBanner({
  run,
  detail,
  question,
  evidence,
  evidencePending,
}: {
  run: WfRun;
  detail?: string;
  /** The pending human `ask` message when `paused_reason === "question"`. */
  question?: WfMessage;
  /** The review evidence for a `paused(approval)` run (its `gate_evidence`
   *  event), or `null` when none has arrived. */
  evidence?: GateEvidence | null;
  /** The journal is still loading — absent evidence may yet arrive. */
  evidencePending?: boolean;
}) {
  const [busy, setBusy] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const setLastError = useAppStore((s) => s.setLastError);

  const isApproval = run.status === "paused" && run.paused_reason === "approval";

  // The review is only meaningful while the run is paused on approval; when the
  // state flips underneath it (approved elsewhere, reject exhausted the budget),
  // an open modal would review a run that no longer awaits one — close it.
  useEffect(() => {
    if (!isApproval) setReviewing(false);
  }, [isApproval]);

  const spec = specFor(run, detail);
  if (!spec) return null;

  const onAct = async (action: Action) => {
    if (busy) return;
    setBusy(true);
    try {
      await action.run(run.id);
      // The row updates via the `wf:run` subscription; nothing to do on success.
    } catch (e) {
      // Surface the failure inline rather than leaving a silent no-op.
      setLastError(`Action failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const paused = run.status === "paused";
  const isQuestion = paused && run.paused_reason === "question";
  const isBudget = paused && run.paused_reason === "budget_exceeded";

  return (
    <div className={`wf-banner ${spec.tone}`}>
      <span className="wf-banner-icon">
        <Icon name={spec.tone === "danger" ? "close" : "pause"} size={14} />
      </span>
      <div className="wf-banner-text">
        <div className="wf-banner-title">{spec.title}</div>
        <div className="wf-banner-body">{spec.body}</div>
        {isQuestion && <AnswerForm run={run} question={question} onError={setLastError} />}
        {isBudget && <ResumeBudgetForm run={run} onError={setLastError} />}
      </div>
      <div className="wf-banner-actions">
        {isApproval && (
          <button type="button" className="btn-t primary" onClick={() => setReviewing(true)}>
            <Icon name="diff" size={13} /> Review changes…
          </button>
        )}
        {spec.actions.map((a) => (
          <button
            key={a.label}
            type="button"
            className={`btn-t ${a.primary ? "primary" : "outline"}`}
            disabled={busy}
            onClick={() => void onAct(a)}
          >
            <Icon name={a.icon} size={13} /> {a.label}
          </button>
        ))}
      </div>
      {reviewing && (
        <ReviewModal
          run={run}
          evidence={evidence ?? null}
          pending={evidencePending ?? false}
          onClose={() => setReviewing(false)}
          onError={setLastError}
        />
      )}
    </div>
  );
}
