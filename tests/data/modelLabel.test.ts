import { describe, expect, it } from "vitest";

import { prettyModelLabel } from "@/data/modelLabel";

describe("prettyModelLabel", () => {
  it("formats Claude ids", () => {
    expect(prettyModelLabel("claude-opus-4-7")).toBe("Claude Opus 4.7");
    expect(prettyModelLabel("claude-sonnet-4-6")).toBe("Claude Sonnet 4.6");
    expect(prettyModelLabel("claude-haiku-4-5")).toBe("Claude Haiku 4.5");
    expect(prettyModelLabel("claude-opus-4-8")).toBe("Claude Opus 4.8");
  });

  it("drops a trailing date snapshot", () => {
    expect(prettyModelLabel("claude-haiku-4-5-20251001")).toBe("Claude Haiku 4.5");
  });

  it("formats GPT / Codex ids", () => {
    expect(prettyModelLabel("gpt-5.5")).toBe("GPT-5.5");
    expect(prettyModelLabel("gpt-5.2-codex")).toBe("GPT-5.2 Codex");
  });

  it("keeps the o-series lowercase convention", () => {
    expect(prettyModelLabel("o4-mini")).toBe("o4-mini");
  });

  it("formats Gemini and Grok ids", () => {
    expect(prettyModelLabel("gemini-3-pro")).toBe("Gemini 3 Pro");
    expect(prettyModelLabel("grok-code")).toBe("Grok Code");
  });

  it("leaves unknown/routed model ids unchanged", () => {
    expect(prettyModelLabel("big-pickle")).toBe("big-pickle");
    expect(prettyModelLabel("deepseek-v4-flash-free")).toBe("deepseek-v4-flash-free");
  });

  it("handles empty input", () => {
    expect(prettyModelLabel("")).toBe("");
  });
});
