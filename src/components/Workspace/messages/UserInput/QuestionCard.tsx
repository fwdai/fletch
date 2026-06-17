import { useEffect, useRef, useState } from "react";
import { Icon } from "../../../Icon";
import { Markdown } from "../../../Markdown";
import type { UIAnswer, UIQuestion } from "./parse";

/** One question rendered as the prototype's `.m-q` card. Live until answered;
 *  once `committed` it folds into a quiet resolved summary. Single-select
 *  answers immediately on click; multiSelect collects picks behind a Confirm;
 *  "Something else…" opens an inline free-text composer. */
export function QuestionCard({
  question,
  index,
  total,
  committed,
  answer,
  onAnswer,
}: {
  question: UIQuestion;
  index: number;
  total: number;
  /** True once the answer has been sent to the agent — render resolved, no edits. */
  committed: boolean;
  /** The chosen answer, when one exists (live multi-question or committed). */
  answer: UIAnswer | null;
  onAnswer: (answer: UIAnswer) => void;
}) {
  const opts = question.options;
  const [picks, setPicks] = useState<Set<string>>(new Set());
  const [otherOpen, setOtherOpen] = useState(false);
  const [otherText, setOtherText] = useState("");
  const otherRef = useRef<HTMLTextAreaElement>(null);

  const answerOne = (label: string) => onAnswer({ labels: [label], isOther: false });
  const confirmMulti = () => {
    if (picks.size === 0) return;
    onAnswer({
      labels: opts.filter((o) => picks.has(o.id)).map((o) => o.label),
      isOther: false,
    });
  };
  const answerOther = () => {
    const t = otherText.trim();
    if (t) onAnswer({ labels: [t], isOther: true });
  };

  // Number keys (1..n) pick an option — only for a lone single-select question,
  // so shortcuts don't clash across stacked cards. Disabled once committed.
  useEffect(() => {
    if (committed || total !== 1 || question.multiSelect) return;
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const n = Number.parseInt(e.key, 10);
      if (n >= 1 && n <= opts.length) {
        e.preventDefault();
        answerOne(opts[n - 1].label);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [committed, total, question.multiSelect, opts]);

  useEffect(() => {
    if (otherOpen) otherRef.current?.focus();
  }, [otherOpen]);

  // ── resolved ──────────────────────────────────────────────────────
  if (committed && answer) {
    return (
      <div className="m-q is-answered">
        <div className="q-head">
          <span className="q-label resolved">
            <Icon name="check" size={11} /> Answered
          </span>
          {question.header && <span className="q-ctx">{question.header}</span>}
        </div>
        <div className="q-prompt resolved">{question.prompt}</div>
        <div className="q-answer">
          <span className="qa-mark">
            <Icon name="check" size={10} />
          </span>
          <span className={`qa-text ${answer.isOther ? "other" : ""}`}>
            {answer.labels.join(", ")}
          </span>
        </div>
      </div>
    );
  }

  // ── live ──────────────────────────────────────────────────────────
  return (
    <div className="m-q">
      <div className="q-head">
        <span className="q-label">
          <Icon name="sparkle" size={11} /> Needs your input
          {total > 1 && (
            <span className="q-step">
              {index + 1}/{total}
            </span>
          )}
        </span>
        {question.header && <span className="q-ctx">{question.header}</span>}
      </div>
      <div className="q-prompt">{question.prompt}</div>
      {question.body && (
        <div className="q-body">
          <Markdown>{question.body}</Markdown>
        </div>
      )}

      <div className="q-opts">
        {opts.map((o, i) => {
          const checked = picks.has(o.id);
          return (
            <button
              key={o.id}
              type="button"
              className={`q-opt ${checked ? "is-checked" : ""}`}
              style={{ animationDelay: `${0.04 * i + 0.02}s` }}
              onClick={() => {
                if (question.multiSelect) {
                  setPicks((prev) => {
                    const next = new Set(prev);
                    if (next.has(o.id)) next.delete(o.id);
                    else next.add(o.id);
                    return next;
                  });
                } else {
                  answerOne(o.label);
                }
              }}
            >
              <span className={`q-radio ${question.multiSelect ? "check" : ""}`}>
                {question.multiSelect && checked && <Icon name="check" size={10} />}
              </span>
              <span className="q-opt-body">
                <span className="q-opt-top">
                  <span className="q-opt-label">{o.label}</span>
                  {o.recommended && <span className="q-rec">Recommended</span>}
                </span>
                {o.desc && <span className="q-opt-desc">{o.desc}</span>}
              </span>
              {!question.multiSelect && <span className="q-kbd">{i + 1}</span>}
            </button>
          );
        })}

        {question.allowOther && !otherOpen && (
          <button
            type="button"
            className="q-opt q-other-trigger"
            style={{ animationDelay: `${0.04 * opts.length + 0.02}s` }}
            onClick={() => setOtherOpen(true)}
          >
            <span className="q-radio plus">
              <Icon name="plus" size={10} />
            </span>
            <span className="q-opt-body">
              <span className="q-opt-label muted">Something else…</span>
              <span className="q-opt-desc">Write your own answer for the agent.</span>
            </span>
          </button>
        )}

        {question.allowOther && otherOpen && (
          <div className="q-other">
            <textarea
              ref={otherRef}
              className="q-other-input"
              placeholder="Tell the agent what you'd prefer…"
              value={otherText}
              onChange={(e) => setOtherText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  answerOther();
                }
                if (e.key === "Escape") {
                  setOtherOpen(false);
                  setOtherText("");
                }
              }}
            />
            <div className="q-other-foot">
              <span className="q-other-hint">
                <span className="kbd">↵</span> to send
              </span>
              <span className="grow" />
              <button
                type="button"
                className="q-other-cancel"
                onClick={() => {
                  setOtherOpen(false);
                  setOtherText("");
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                className="q-other-send"
                disabled={!otherText.trim()}
                onClick={answerOther}
              >
                Send <Icon name="arrowUp" size={11} />
              </button>
            </div>
          </div>
        )}

        {question.multiSelect && (
          <div className="q-multi-foot">
            <button
              type="button"
              className="q-other-send"
              disabled={picks.size === 0}
              onClick={confirmMulti}
            >
              Confirm{picks.size > 0 ? ` (${picks.size})` : ""}{" "}
              <Icon name="arrowUp" size={11} />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
