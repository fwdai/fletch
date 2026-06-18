import { describe, it, expect } from "vitest";
import {
  isUserInputTool,
  parseUserInput,
  formatAnswer,
  type UIAnswer,
} from "./parse";

describe("isUserInputTool", () => {
  it("matches the two question tools and nothing else", () => {
    expect(isUserInputTool("AskUserQuestion")).toBe(true);
    expect(isUserInputTool("ExitPlanMode")).toBe(true);
    expect(isUserInputTool("Bash")).toBe(false);
    expect(isUserInputTool("askuserquestion")).toBe(false); // case-sensitive
  });
});

describe("parseUserInput — AskUserQuestion", () => {
  it("normalizes a single question with options", () => {
    const model = parseUserInput("AskUserQuestion", {
      questions: [
        {
          question: "Which database should we use?",
          header: "Storage",
          options: [
            { label: "Postgres", description: "Relational", recommended: true },
            { label: "SQLite" },
          ],
        },
      ],
    });
    expect(model.tool).toBe("AskUserQuestion");
    expect(model.questions).toHaveLength(1);
    const q = model.questions[0];
    expect(q.prompt).toBe("Which database should we use?");
    expect(q.header).toBe("Storage");
    expect(q.multiSelect).toBe(false);
    expect(q.allowOther).toBe(true);
    expect(q.options[0]).toMatchObject({
      label: "Postgres",
      desc: "Relational",
      recommended: true,
    });
    expect(q.options[1]).toMatchObject({ label: "SQLite", recommended: false });
  });

  it("carries multiSelect through and handles multiple questions", () => {
    const model = parseUserInput("AskUserQuestion", {
      questions: [
        { question: "Pick languages", multiSelect: true, options: [{ label: "TS" }] },
        { question: "Pick a license", options: [{ label: "MIT" }] },
      ],
    });
    expect(model.questions).toHaveLength(2);
    expect(model.questions[0].multiSelect).toBe(true);
    expect(model.questions[1].multiSelect).toBe(false);
  });

  it("falls back to a flat {question, options} shape", () => {
    const model = parseUserInput("AskUserQuestion", {
      question: "Proceed?",
      options: [{ label: "Yes" }, { label: "No" }],
    });
    expect(model.questions).toHaveLength(1);
    expect(model.questions[0].prompt).toBe("Proceed?");
    expect(model.questions[0].options).toHaveLength(2);
  });

  it("never throws on garbage input", () => {
    expect(parseUserInput("AskUserQuestion", null).questions).toEqual([]);
    expect(parseUserInput("AskUserQuestion", { questions: "nope" }).questions).toEqual(
      [],
    );
  });
});

describe("parseUserInput — ExitPlanMode", () => {
  it("renders the plan as a body with approve/reject options", () => {
    const model = parseUserInput("ExitPlanMode", { plan: "1. Do thing\n2. Profit" });
    expect(model.tool).toBe("ExitPlanMode");
    expect(model.questions).toHaveLength(1);
    const q = model.questions[0];
    expect(q.body).toBe("1. Do thing\n2. Profit");
    expect(q.options.map((o) => o.label)).toEqual(["Approve & proceed", "Keep planning"]);
    expect(q.options[0].recommended).toBe(true);
    expect(q.allowOther).toBe(true);
  });
});

describe("formatAnswer", () => {
  const opt = (label: string): UIAnswer => ({ labels: [label], isOther: false });

  it("sends just the answer for a single question", () => {
    const model = parseUserInput("AskUserQuestion", {
      questions: [{ question: "DB?", options: [{ label: "Postgres" }] }],
    });
    expect(formatAnswer(model, [opt("Postgres")])).toBe("Postgres");
  });

  it("joins multiSelect labels with commas", () => {
    const model = parseUserInput("AskUserQuestion", {
      questions: [{ question: "Langs?", multiSelect: true, options: [] }],
    });
    expect(
      formatAnswer(model, [{ labels: ["TS", "Rust"], isOther: false }]),
    ).toBe("TS, Rust");
  });

  it("labels each line for multi-question calls", () => {
    const model = parseUserInput("AskUserQuestion", {
      questions: [
        { question: "DB?", header: "Storage", options: [] },
        { question: "License?", header: "License", options: [] },
      ],
    });
    const out = formatAnswer(model, [opt("Postgres"), opt("MIT")]);
    expect(out).toBe("Storage: Postgres\nLicense: MIT");
  });

  it("approves or keeps planning for ExitPlanMode", () => {
    const model = parseUserInput("ExitPlanMode", { plan: "x" });
    expect(formatAnswer(model, [opt("Approve & proceed")])).toBe(
      "Approved. Proceed with the plan.",
    );
    expect(formatAnswer(model, [opt("Keep planning")])).toBe(
      "Not yet — keep planning.",
    );
  });

  it("includes free-text feedback when keeping planning", () => {
    const model = parseUserInput("ExitPlanMode", { plan: "x" });
    const out = formatAnswer(model, [
      { labels: ["Use a queue instead"], isOther: true },
    ]);
    expect(out).toBe("Not yet — keep planning. Use a queue instead");
  });
});
