// RunView/PausedBanner.tsx — the paused/failed banner (spec §14.2). Every state a
// run can rest in names its cause in plain language and offers its action:
// approve (S4), retry/resume (S4), answer a question (S10), and conflict
// resolution (S9). No dead buttons — each paused reason wires to its command.

import { useCallback, useEffect, useState } from "react";
import { api, type GateEvidence, type WfMessage, type WfRun } from "../../../api";
import type { IconName } from "../../../components/Icon";
import { Icon } from "../../../components/Icon";
import { ReviewSurface } from "../../../components/ReviewSurface";
import { IconButton, Scrim } from "../../../components/ui";
import { useAppStore } from "../../../store";
import type { Budgets } from "../../spec";
import { pausedLabel } from "../status";

interface Action {
  label: string;
  icon: IconName;
  run: (runId: string) => Promise<void>;
  primary?: boolean;
}

interface BannerSpec {
  tone: "amber" | "danger";
  title: string;
  body: string;
  actions: Action[];
}

const RETRY: Action = { label: "Retry", icon: "refresh", run: api.wfRetry, primary: true };
const RESUME: Action = { label: "Resume", icon: "play", run: api.wfResume, primary: true };
// Conflict resolution (§12.3): let an agent resolve the pinned conflict snapshot,
// or continue after the user has resolved it in the run repo's integration
// worktree and committed.
const RESOLVE_AGENT: Action = {
  label: "Resolve with agent",
  icon: "play",
  run: (id) => api.wfResolveConflict(id, "agent"),
  primary: true,
};
const RESOLVE_HUMAN: Action = {
  label: "I resolved it — continue",
  icon: "check",
  run: (id) => api.wfResolveConflict(id, "human"),
};

function specFor(run: WfRun, detail?: string): BannerSpec | null {
  if (run.status === "failed") {
    return {
      tone: "danger",
      title: "Run failed",
      body: run.error || "The run stopped with an unrecoverable error.",
      actions: [],
    };
  }
  if (run.status !== "paused" || !run.paused_reason) return null;

  const title = `Paused — ${pausedLabel(run.paused_reason)}`;
  switch (run.paused_reason) {
    case "approval":
      // The action is the review surface (rendered inline below), not a bare
      // Approve button — a merge decision deserves the evidence first.
      return {
        tone: "amber",
        title,
        body: detail || "A step is waiting for your review before handing off to the next.",
        actions: [],
      };
    case "blocked_gate":
      return {
        tone: "amber",
        title,
        body: detail || "A step finished but its gate isn't satisfied yet.",
        actions: [RETRY],
      };
    case "stalled":
      return {
        tone: "amber",
        title,
        body: detail || "A step stopped making progress and didn't recover after a nudge.",
        actions: [RETRY, RESUME],
      };
    case "budget_exceeded":
      return {
        tone: "amber",
        title,
        body: detail || "The run reached a configured budget. Raise it to keep going.",
        // The resume form (rendered in the body) raises the caps and re-drives;
        // a plain Resume would just re-hit the same budget.
        actions: [],
      };
    case "question":
      return {
        tone: "amber",
        title,
        body: detail || "A step asked a question and is waiting for your answer.",
        actions: [],
      };
    case "conflict":
      return {
        tone: "amber",
        title,
        body:
          detail ||
          "Merging parallel work hit a conflict. Let an agent resolve it, or resolve it " +
            "yourself in the run's integration worktree and continue.",
        actions: [RESOLVE_AGENT, RESOLVE_HUMAN],
      };
  }
}

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

/** The approval review, framed as a modal over the run. Wires the shared
 *  `ReviewSurface` to this run's diff source and approve/reject commands; the
 *  surface itself is store-agnostic so a fleet queue can mount it elsewhere. */
function ReviewModal({
  run,
  evidence,
  pending,
  onClose,
  onError,
}: {
  run: WfRun;
  evidence: GateEvidence | null;
  pending: boolean;
  onClose: () => void;
  onError: (message: string) => void;
}) {
  const getDiff = useCallback(
    (path: string | null): Promise<string> => {
      if (!evidence) return Promise.resolve("");
      return api.wfRunDiff(run.id, evidence.base_sha, evidence.head_sha, path ?? undefined);
    },
    [run.id, evidence],
  );

  // Close on success; the run-state effect above also catches external flips.
  const decide = useCallback(
    async (action: Promise<void>) => {
      await action;
      onClose();
    },
    [onClose],
  );

  return (
    <>
      <Scrim onClose={onClose} zIndex={399} />
      <div className="rv-modal">
        <div className="rv-modal-card">
          <div className="rv-modal-head">
            <span className="rv-modal-title">
              <Icon name="combine" size={14} style={{ color: "var(--accent)" }} /> Review
              <span className="rv-modal-step">{run.task || run.name}</span>
            </span>
            <IconButton className="rv-modal-close" tip="Close" onClick={onClose}>
              <Icon name="close" size={14} />
            </IconButton>
          </div>
          <ReviewSurface
            evidence={evidence}
            pending={pending}
            getDiff={getDiff}
            onApprove={() => decide(api.wfApprove(run.id))}
            onReject={(note) => decide(api.wfReject(run.id, note))}
            onError={onError}
          />
        </div>
      </div>
    </>
  );
}

/** Inline answer form for a `paused(question)` run: shows the step's question
 *  and options (if any) and delivers the reply via `wf_answer`, which resumes
 *  the run (§10.4). */
function AnswerForm({
  run,
  question,
  onError,
}: {
  run: WfRun;
  question?: WfMessage;
  onError: (m: string) => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const q = (question?.body ?? null) as { question?: string; options?: string[] } | null;

  const send = async () => {
    const body = text.trim();
    if (!body || busy || !question) return;
    setBusy(true);
    try {
      await api.wfAnswer(run.project_id, run.id, question.id, body);
      // The `wf:run` subscription flips the run back to running; nothing else.
    } catch (e) {
      onError(`Answer failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  if (!question) {
    // Paused on a question but the message hasn't loaded yet — never a dead end.
    return <div className="wf-answer-hint">Loading the question…</div>;
  }

  return (
    <div className="wf-answer">
      {q?.question && <div className="wf-answer-q">“{q.question}”</div>}
      {q?.options && q.options.length > 0 && (
        <div className="wf-answer-opts">
          {q.options.map((opt) => (
            <button
              key={opt}
              type="button"
              className="btn-t outline"
              disabled={busy}
              onClick={() => setText(opt)}
            >
              {opt}
            </button>
          ))}
        </div>
      )}
      <div className="wf-answer-row">
        <textarea
          className="wf-answer-input"
          placeholder="Type your answer…"
          value={text}
          disabled={busy}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button
          type="button"
          className="btn-t primary"
          disabled={busy || !text.trim()}
          onClick={() => void send()}
        >
          <Icon name="check" size={13} /> Send
        </button>
      </div>
    </div>
  );
}

/** Does the run have a token cap set? Token patches are ignored when the run's
 *  token budget is unlimited (§11.2), so the field is only worth showing then. */
function hasTokenCap(budgets: unknown): boolean {
  if (!budgets || typeof budgets !== "object" || !("tokens" in budgets)) return false;
  const t = (budgets as { tokens: unknown }).tokens;
  return typeof t === "number" && t > 0;
}

/** Inline "raise the budget and resume" form for a `paused(budget_exceeded)`
 *  run (§11.2). Each field is an additive bump to a run-level cap; resuming with
 *  no bump would just re-hit the same budget, so at least one is required. */
function ResumeBudgetForm({ run, onError }: { run: WfRun; onError: (m: string) => void }) {
  const [turns, setTurns] = useState("");
  const [tokens, setTokens] = useState("");
  const [minutes, setMinutes] = useState("");
  const [busy, setBusy] = useState(false);
  const showTokens = hasTokenCap(run.budgets);

  const parse = (s: string): number | undefined => {
    const n = Math.floor(Number(s));
    return Number.isFinite(n) && n > 0 ? n : undefined;
  };
  const patch: Budgets = {
    turns: parse(turns),
    tokens: showTokens ? parse(tokens) : undefined,
    wall_clock_mins: parse(minutes),
  };
  const hasBump = patch.turns != null || patch.tokens != null || patch.wall_clock_mins != null;

  const resume = async () => {
    if (busy || !hasBump) return;
    setBusy(true);
    try {
      await api.wfResume(run.id, patch);
      // The `wf:run` subscription flips the run back to running on success.
    } catch (e) {
      onError(`Resume failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const field = (label: string, value: string, set: (v: string) => void) => (
    <label className="wf-budget-field">
      <span>+ {label}</span>
      <input
        type="number"
        min="0"
        step="1"
        inputMode="numeric"
        placeholder="0"
        value={value}
        disabled={busy}
        onChange={(e) => set(e.target.value)}
      />
    </label>
  );

  return (
    <div className="wf-budget-patch">
      {field("turns", turns, setTurns)}
      {showTokens && field("tokens", tokens, setTokens)}
      {field("minutes", minutes, setMinutes)}
      <button
        type="button"
        className="btn-t primary"
        disabled={busy || !hasBump}
        onClick={() => void resume()}
      >
        <Icon name="play" size={13} /> Resume
      </button>
    </div>
  );
}
