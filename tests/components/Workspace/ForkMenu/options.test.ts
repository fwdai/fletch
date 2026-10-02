import { describe, expect, it } from "vitest";
import {
  codeChoices,
  contextChoices,
  DEFAULT_CODE,
  defaultContext,
} from "@/components/Workspace/ForkMenu/options";
import { PROVIDERS } from "@/data/providers";

const labels = <T>(choices: { value: T; label: string }[]) =>
  choices.map((c) => [c.value, c.label]);

describe("fork menu choices", () => {
  it("offers a message fork the full conversation first, then a summary, and every code", () => {
    expect(labels(contextChoices("message", "claude"))).toEqual([
      ["full", "Full conversation up to here"],
      ["summary", "Summary up to here"],
    ]);
    expect(labels(codeChoices("message"))).toEqual([
      ["clean", "Clean from base"],
      ["current", "Current code"],
      ["at_message", "Code as of this message"],
    ]);
  });

  it("offers a conversation fork no code as of a message: it has none", () => {
    expect(labels(contextChoices("conversation", "claude"))).toEqual([
      ["full", "Full conversation"],
      ["summary", "Summary of the conversation"],
    ]);
    expect(labels(codeChoices("conversation"))).toEqual([
      ["clean", "Clean from base"],
      ["current", "Current code"],
    ]);
  });

  it("never offers a fresh conversation: every fork continues this one", () => {
    for (const scope of ["message", "conversation"] as const) {
      for (const { id } of PROVIDERS) {
        expect(contextChoices(scope, id).map((c) => c.value)).toEqual(["full", "summary"]);
      }
    }
  });

  it("carries the full conversation for the providers Fletch writes transcripts for", () => {
    const full = (provider: string) =>
      contextChoices("message", provider).find((c) => c.value === "full");
    // The backend's `transcript_writer` set.
    for (const provider of ["claude", "codex", "pi"]) {
      expect(full(provider)?.reason).toBeNull();
      expect(defaultContext(provider)).toBe("full");
    }
    for (const provider of ["cursor", "antigravity", "opencode"]) {
      expect(full(provider)?.reason).toMatch(/can only carry a summary/);
      expect(defaultContext(provider)).toBe("summary");
    }
    expect(full("cursor")?.reason).toBe("Cursor Agent can only carry a summary.");
    // A summary is always available.
    for (const { id } of PROVIDERS) {
      expect(contextChoices("message", id).find((c) => c.value === "summary")?.reason).toBeNull();
    }
  });

  it("opens on a selection every scope offers and can pick", () => {
    for (const scope of ["message", "conversation"] as const) {
      for (const { id } of PROVIDERS) {
        const opened = contextChoices(scope, id).find((c) => c.value === defaultContext(id));
        expect(opened?.reason).toBeNull();
      }
      expect(codeChoices(scope).map((c) => c.value)).toContain(DEFAULT_CODE);
    }
  });
});
