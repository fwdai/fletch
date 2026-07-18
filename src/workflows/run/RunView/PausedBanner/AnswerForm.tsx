import { useState } from "react";
import { api, type WfMessage, type WfRun } from "../../../../api";
import { Icon } from "../../../../components/Icon";

/** Inline answer form for a `paused(question)` run: shows the step's question
 *  and options (if any) and delivers the reply via `wf_answer`, which resumes
 *  the run (§10.4). */
export function AnswerForm({
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
