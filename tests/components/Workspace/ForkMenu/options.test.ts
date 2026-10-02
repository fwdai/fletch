import { describe, expect, it } from "vitest";
import {
  codeChoices,
  contextChoices,
  DEFAULT_CODE,
  DEFAULT_CONTEXT,
} from "@/components/Workspace/ForkMenu/options";

const labels = <T>(choices: { value: T; label: string }[]) =>
  choices.map((c) => [c.value, c.label]);

describe("fork menu choices", () => {
  it("offers a message fork every code, its own code as of the message included", () => {
    expect(labels(contextChoices("message"))).toEqual([
      ["none", "Fresh conversation"],
      ["summary", "Summary up to here"],
    ]);
    expect(labels(codeChoices("message"))).toEqual([
      ["clean", "Clean from base"],
      ["current", "Current code"],
      ["at_message", "Code as of this message"],
    ]);
  });

  it("offers a conversation fork no code as of a message: it has none", () => {
    expect(labels(contextChoices("conversation"))).toEqual([
      ["none", "Fresh conversation"],
      ["summary", "Summary of the conversation"],
    ]);
    expect(labels(codeChoices("conversation"))).toEqual([
      ["clean", "Clean from base"],
      ["current", "Current code"],
    ]);
  });

  it("opens on a selection every scope offers", () => {
    for (const scope of ["message", "conversation"] as const) {
      expect(contextChoices(scope).map((c) => c.value)).toContain(DEFAULT_CONTEXT);
      expect(codeChoices(scope).map((c) => c.value)).toContain(DEFAULT_CODE);
    }
  });
});
