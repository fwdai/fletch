// Normalizes the two Claude tools that pause to ask the user — `AskUserQuestion`
// (structured multiple-choice) and `ExitPlanMode` (plan approval) — into one
// model the widget renders, plus the inverse: turning the user's picks back
// into the `tool_result` text we feed to Claude's stdin to unblock the turn.
//
// Only Claude surfaces these in the custom-view JSON stream today; every other
// provider Quorum drives runs fully auto-approved (see the adapter map). The
// model is provider-agnostic, so any tool reported under these names renders
// the widget without per-adapter branching.

export const USER_INPUT_TOOLS = ["AskUserQuestion", "ExitPlanMode"] as const;
export type UserInputTool = (typeof USER_INPUT_TOOLS)[number];

export function isUserInputTool(name: string): name is UserInputTool {
  return (USER_INPUT_TOOLS as readonly string[]).includes(name);
}

export interface UIOption {
  id: string;
  label: string;
  desc?: string;
  recommended?: boolean;
}

export interface UIQuestion {
  id: string;
  /** Short context tag shown in the card header (AskUserQuestion `header`). */
  header?: string;
  /** The question text itself. */
  prompt: string;
  /** Optional markdown body shown above the options (the ExitPlanMode plan). */
  body?: string;
  options: UIOption[];
  multiSelect: boolean;
  /** Whether to offer the free-text "Something else…" escape hatch. */
  allowOther: boolean;
}

export interface UserInputModel {
  tool: UserInputTool;
  questions: UIQuestion[];
}

/** One answer per question: the chosen option labels (one unless multiSelect),
 *  or free-text when the user took the "Something else…" path. */
export interface UIAnswer {
  labels: string[];
  /** Original option ids for non-free-text answers, used for tool semantics. */
  optionIds?: string[];
  /** True when the labels came from the free-text composer, not an option. */
  isOther: boolean;
}

function asRecord(v: unknown): Record<string, unknown> {
  return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
}

function asString(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

function parseOptions(raw: unknown): UIOption[] {
  if (!Array.isArray(raw)) return [];
  return raw.map((o, i) => {
    const r = asRecord(o);
    return {
      id: asString(r.id) ?? `opt-${i}`,
      label: asString(r.label) ?? asString(r.title) ?? `Option ${i + 1}`,
      desc: asString(r.description) ?? asString(r.desc),
      recommended: r.recommended === true,
    };
  });
}

function parseAskUserQuestion(input: Record<string, unknown>): UIQuestion[] {
  const raw = Array.isArray(input.questions) ? input.questions : [];
  const questions = raw.map((q, i): UIQuestion => {
    const r = asRecord(q);
    return {
      id: asString(r.id) ?? `q-${i}`,
      header: asString(r.header),
      prompt: asString(r.question) ?? asString(r.prompt) ?? "",
      options: parseOptions(r.options),
      multiSelect: r.multiSelect === true,
      allowOther: true,
    };
  });
  // Defensive fallback: a flat {question, options} shape with no `questions[]`.
  if (questions.length === 0 && (input.question || input.options)) {
    return [
      {
        id: "q-0",
        header: asString(input.header),
        prompt: asString(input.question) ?? "",
        options: parseOptions(input.options),
        multiSelect: input.multiSelect === true,
        allowOther: true,
      },
    ];
  }
  return questions;
}

function parseExitPlanMode(input: Record<string, unknown>): UIQuestion[] {
  const plan = asString(input.plan) ?? "";
  return [
    {
      id: "plan",
      header: "Plan review",
      prompt: "Ready to proceed with this plan?",
      body: plan || undefined,
      options: [
        { id: "approve", label: "Approve & proceed", recommended: true },
        { id: "reject", label: "Keep planning" },
      ],
      multiSelect: false,
      allowOther: true,
    },
  ];
}

export function parseUserInput(
  tool: UserInputTool,
  rawInput: unknown,
): UserInputModel {
  const input = asRecord(rawInput);
  const questions =
    tool === "ExitPlanMode"
      ? parseExitPlanMode(input)
      : parseAskUserQuestion(input);
  return { tool, questions };
}

/** Reconstruct per-question answers from the tool_result the CLI writes once an
 *  AskUserQuestion is answered — format: `… "Question"="Answer", "Q2"="A2". …`.
 *  Lets a widget rebuilt from the transcript show the same clean answer chips as
 *  the live session, instead of the raw sentence. Returns one entry per question
 *  (null where unmatched), or null if nothing parsed. An answer that isn't one
 *  of the question's option labels is flagged `isOther` (free text). */
export function answersFromResultText(
  model: UserInputModel,
  text: string,
): (UIAnswer | null)[] | null {
  const pairs: Record<string, string> = {};
  const re = /"([^"]+)"\s*=\s*"([^"]*)"/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) pairs[m[1]] = m[2];

  let matched = false;
  const answers = model.questions.map((q): UIAnswer | null => {
    const value = pairs[q.prompt];
    if (value == null) return null;
    matched = true;
    return {
      labels: [value],
      isOther: !q.options.some((o) => o.label === value),
    };
  });
  return matched ? answers : null;
}

/** Build the `answers` map for an `AskUserQuestion` tool result: keys are the
 *  original question text, values the chosen option label(s) (an array for
 *  multiSelect, the user's free text for an "other" answer). This is merged
 *  into the tool's input and returned to the model via the control protocol. */
export function buildAnswers(
  model: UserInputModel,
  answers: UIAnswer[],
): Record<string, string | string[]> {
  const out: Record<string, string | string[]> = {};
  model.questions.forEach((q, i) => {
    const a = answers[i];
    if (!a) return;
    out[q.prompt] = q.multiSelect ? a.labels : a.labels.join(", ");
  });
  return out;
}

/** Build the answer text sent back to the agent as the next user message.
 *  The CLI auto-rejects AskUserQuestion in headless mode before we could return
 *  a real tool_result (see UserInput/index.tsx), so the answer rides in as a
 *  normal turn instead. Single-question calls send just the answer;
 *  multi-question calls label each line so the model can tell them apart. */
export function formatAnswer(
  model: UserInputModel,
  answers: UIAnswer[],
): string {
  if (model.tool === "ExitPlanMode") {
    const a = answers[0];
    const picked = a?.optionIds?.[0] ?? a?.labels[0] ?? "";
    if (a?.isOther) {
      return `Not yet — keep planning. ${a.labels.join(" ")}`.trim();
    }
    return picked === "approve" || picked === "Approve & proceed"
      ? "Approved. Proceed with the plan."
      : "Not yet — keep planning.";
  }

  const lines = model.questions.map((q, i) => {
    const a = answers[i];
    const value = a ? a.labels.join(", ") : "";
    if (model.questions.length === 1) return value;
    const tag = q.header || q.prompt;
    return `${tag}: ${value}`;
  });
  return lines.join("\n");
}
