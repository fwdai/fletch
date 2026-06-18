import { useMemo, useState } from "react";
import { Icon } from "../../../Icon";
import { useAppStore } from "../../../../store";
import type { ToolCall, ToolResult } from "../presenters/types";
import { QuestionCard } from "./QuestionCard";
import {
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
        // True pause: return the structured answer as the tool result.
        const rawInput =
          call.input && typeof call.input === "object"
            ? (call.input as Record<string, unknown>)
            : {};
        void answerToolUse(agentId, call.id, {
          ...rawInput,
          answers: buildAnswers(model, ordered),
        });
      } else {
        // No held prompt (replayed history) — answer as an ordinary message.
        void sendUserMessage(agentId, formatAnswer(model, ordered));
      }
    }
  };

  if (model.questions.length === 0) return null;

  // A genuine (non-error) tool_result means the question was actually answered
  // through it — show a compact summary. The CLI's `is_error` auto-rejection
  // (headless mode) is NOT a real answer, so we fall through to live options.
  if (!committedLocally && result && !result.is_error) {
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
          committed={committedLocally}
          answer={answers[i] ?? null}
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
