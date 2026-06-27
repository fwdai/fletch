import { useMemo, useState } from "react";
import { Icon } from "../../../Icon";
import { useAppStore } from "../../../../store";
import type { ToolCall, ToolResult } from "../presenters/types";
import { QuestionCard } from "./QuestionCard";
import {
  answersFromResultText,
  buildAnswers,
  formatAnswer,
  parseUserInput,
  type UIAnswer,
  type UserInputTool,
} from "./parse";

/** Rich widget for the agent's "needs your input" tools (Claude's
 *  `AskUserQuestion` / `ExitPlanMode`). Renders one card per question; once
 *  every question is answered it delivers the answer to the agent.
 *
 *  Two answer routes:
 *   - **True pause** (live): when the backend is holding this tool's
 *     `can_use_tool` prompt open (a `request_id` for `call.id` exists in
 *     `pendingToolUse`), the agent is genuinely suspended. We answer via
 *     `answerToolUse`, which returns the selection as the tool result and
 *     resumes the turn — no failed tool call, no redundant text.
 *   - **Fallback** (replayed/legacy history, no held prompt): send the answer
 *     as an ordinary user message so it still reaches the model.
 *
 *  A genuine (non-error) `result` folds the widget into a resolved summary. */
export function UserInput({
  tool,
  call,
  result,
  agentId,
}: {
  tool: UserInputTool;
  call: ToolCall;
  result: ToolResult | null;
  agentId?: string;
}) {
  const model = useMemo(() => parseUserInput(tool, call.input), [tool, call.input]);
  const sendUserMessage = useAppStore((s) => s.sendUserMessage);
  const answerToolUse = useAppStore((s) => s.answerToolUse);
  // The held control-protocol request for this tool call, if the agent is
  // currently paused on it (true-pause path). Undefined for replayed history.
  const pendingRequestId = useAppStore((s) =>
    agentId ? s.pendingToolUse[agentId]?.[call.id] : undefined,
  );

  // Keyed by question index rather than a fixed-length array: the model's
  // question count can grow as the tool_use input streams in, and we must not
  // treat a partially-filled multi-question call as complete.
  const [answers, setAnswers] = useState<Record<number, UIAnswer>>({});
  const [committedLocally, setCommittedLocally] = useState(false);

  const handleAnswer = (i: number, ans: UIAnswer) => {
    const next = { ...answers, [i]: ans };
    setAnswers(next);
    if (model.questions.every((_, idx) => next[idx])) {
      setCommittedLocally(true);
      if (!agentId) return;
      const ordered = model.questions.map((_, idx) => next[idx]);
      if (pendingRequestId) {
        // True pause: return the structured answer as the tool result. For
        // ExitPlanMode, "Keep planning" is a permission denial, not a tool
        // result approval.
        const rawInput =
          call.input && typeof call.input === "object"
            ? (call.input as Record<string, unknown>)
            : {};
        const keepPlanning =
          model.tool === "ExitPlanMode" &&
          (ordered[0]?.optionIds?.[0] === "reject" || ordered[0]?.isOther);
        void answerToolUse(
          agentId,
          call.id,
          {
            ...rawInput,
            answers: buildAnswers(model, ordered),
          },
          keepPlanning ? "deny" : "allow",
          keepPlanning ? formatAnswer(model, ordered) : undefined,
        );
      } else {
        // No held prompt (replayed history) — answer as an ordinary message.
        void sendUserMessage(agentId, formatAnswer(model, ordered));
      }
    }
  };

  if (model.questions.length === 0) return null;

  // Interrupted: an is_error result with no live prompt means the user hit Stop
  // (the held prompt was denied) — or it's an old pre-feature auto-deny. Either
  // way the agent is no longer waiting, so show a quiet "Dismissed" state rather
  // than answerable options that would imply it still is.
  if (!committedLocally && result?.is_error && !pendingRequestId) {
    return (
      <div className="m-q is-dismissed">
        <div className="q-head">
          <span className="q-label dismissed">
            <Icon name="close" size={11} /> Dismissed
          </span>
        </div>
        {model.questions.map((q) => (
          <div key={q.id} className="q-prompt resolved">
            {q.prompt}
          </div>
        ))}
        <div className="q-dismissed-note">Not answered — you stopped the agent.</div>
      </div>
    );
  }

  // Resolved purely from the transcript (the widget was rebuilt after the turn
  // completed): the CLI's `is_error` auto-rejection is NOT a real answer, so we
  // only treat a genuine result as resolved. Reconstruct the structured answers
  // from the result text so reload shows the same answer chips as the live
  // session rather than the CLI's raw "Your questions have been answered…"
  // sentence.
  const fromTranscript =
    !committedLocally && result && !result.is_error
      ? answersFromResultText(model, resultText(result.content))
      : null;
  const committed = committedLocally || !!fromTranscript;
  // Reflect a locally chosen answer immediately, before the whole widget
  // commits: in a multi-question stack the user may answer questions one at a
  // time, and each card must show its own selection right away rather than
  // staying blank until the last question is answered.
  const answerFor = (i: number): UIAnswer | null =>
    answers[i] ?? (committedLocally ? null : fromTranscript?.[i]) ?? null;

  // Answered, but the result didn't parse into per-question answers — fall back
  // to a compact summary of the raw result text rather than showing nothing.
  if (!committedLocally && result && !result.is_error && !fromTranscript) {
    return (
      <div className="m-q is-answered">
        <div className="q-head">
          <span className="q-label resolved">
            <Icon name="check" size={11} /> Answered
          </span>
        </div>
        {model.questions.map((q) => (
          <div key={q.id} className="q-prompt resolved">
            {q.prompt}
          </div>
        ))}
        <div className="q-answer">
          <span className="qa-mark">
            <Icon name="check" size={10} />
          </span>
          <span className="qa-text">{resultText(result.content)}</span>
        </div>
      </div>
    );
  }

  return (
    <div className="m-q-stack">
      {model.questions.map((q, i) => (
        <QuestionCard
          key={q.id}
          question={q}
          index={i}
          total={model.questions.length}
          committed={committed}
          answer={answerFor(i)}
          onAnswer={(ans) => handleAnswer(i, ans)}
        />
      ))}
    </div>
  );
}

/** Flatten a tool_result's content (string or content-block array) to text. */
function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b) =>
        b && typeof b === "object" && "text" in b
          ? String((b as { text: unknown }).text)
          : "",
      )
      .join("")
      .trim();
  }
  return "";
}
