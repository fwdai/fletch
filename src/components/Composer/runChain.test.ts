import { describe, expect, it } from "vitest";
import { chainTip, compactAccountName, shortModelName } from "./runChain";

describe("shortModelName", () => {
  it("drops the vendor the provider icon already states", () => {
    expect(shortModelName("Claude Fable 5.1")).toBe("Fable 5.1");
    expect(shortModelName("Claude Opus 4.7")).toBe("Opus 4.7");
    expect(shortModelName("claude sonnet 4")).toBe("sonnet 4");
    expect(shortModelName("OpenAI o4-mini")).toBe("o4-mini");
  });

  it("keeps a family name that is not a vendor", () => {
    expect(shortModelName("GPT-5.5 Codex")).toBe("GPT-5.5 Codex");
    expect(shortModelName("Gemini 3 Pro")).toBe("Gemini 3 Pro");
    expect(shortModelName("Default model")).toBe("Default model");
  });

  it("never returns an empty name", () => {
    expect(shortModelName("Claude")).toBe("Claude");
    expect(shortModelName("Claude ")).toBe("Claude ");
  });
});

describe("compactAccountName", () => {
  it("shows an email by its local part", () => {
    expect(compactAccountName("alex@example.com")).toBe("alex");
  });

  it("cuts a long name with an ellipsis", () => {
    expect(compactAccountName("alexander.chaplinsky@example.com")).toBe("alexander.cha…");
    expect(compactAccountName("a-very-long-account-handle")).toBe("a-very-long-a…");
  });

  it("leaves a short name alone and names the terminal login", () => {
    expect(compactAccountName("tttr")).toBe("tttr");
    expect(compactAccountName("  work  ")).toBe("work");
    expect(compactAccountName("Terminal login")).toBe("Terminal");
  });
});

describe("chainTip", () => {
  it("joins the present pieces with a middle dot", () => {
    expect(chainTip("Claude Code", "alex", "Claude Fable 5.1")).toBe(
      "Claude Code · alex · Claude Fable 5.1",
    );
    expect(chainTip("Codex", null, undefined, false, "", "GPT-5.5")).toBe("Codex · GPT-5.5");
  });
});
