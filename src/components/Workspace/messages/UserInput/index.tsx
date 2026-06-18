import { useMemo, useState } from "react";
import { Icon } from "../../../Icon";
import { useAppStore } from "../../../../store";
import type { ToolCall, ToolResult } from "../presenters/types";
import { QuestionCard } from "./QuestionCard";
import {
  formatAnswer,
  parseUserInput,
  type UIAnswer,
  type UserInputTool,
} from "./parse";

/** Rich widget for the agent's "needs your input" tools (Claude's
 *  `AskUserQuestion` / `ExitPlanMode`). Renders one card per question; once
 *  every question is answered it sends the chosen answer back as the next
 *  user message.
 *
 *  Why a user message and not a tool_result: in headless `--print` mode the
 *  Claude CLI has no interactive surface to present the question, so it
 *  auto-rejects the tool with an `is_error` tool_result within milliseconds
 *  and the model continues. By the time we render, that tool_use is already
 *  closed — a tool_result keyed to it would be a duplicate. So we ignore the
 *  CLI's error result, surface the options as quick replies, and let the
 *  user's pick ride in as an ordinary turn, which the model reads and answers.
 *
 *  A genuine (non-error) `result` — e.g. from a future interactive flow —
 *  still folds the widget into a resolved summary. */
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
      if (agentId) {
        const ordered = model.questions.map((_, idx) => next[idx]);
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
