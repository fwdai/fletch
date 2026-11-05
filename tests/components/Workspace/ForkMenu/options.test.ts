import { describe, expect, it } from "vitest";
import {
  CODE_CHOICES,
  contextChoices,
  DEFAULT_CODE,
  defaultContext,
} from "@/components/Workspace/ForkMenu/options";
import { PROVIDERS } from "@/data/providers";

const labels = <T>(choices: { value: T; label: string }[]) =>
  choices.map((c) => [c.value, c.label]);

describe("fork menu choices", () => {
  it("offers the full conversation first, then a summary, and every code", () => {
    expect(labels(contextChoices("claude"))).toEqual([
      ["full", "Full conversation up to here"],
      ["summary", "Summary up to here"],
    ]);
    expect(labels(CODE_CHOICES)).toEqual([
      ["clean", "Clean from base"],
      ["current", "Current code"],
      ["at_message", "Code as of this message"],
    ]);
  });

  it("never offers a fresh conversation: every fork continues this one", () => {
    for (const { id } of PROVIDERS) {
      expect(contextChoices(id).map((c) => c.value)).toEqual(["full", "summary"]);
    }
  });

  it("carries the full conversation for the providers Fletch writes transcripts for", () => {
    const full = (provider: string) => contextChoices(provider).find((c) => c.value === "full");
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
      expect(contextChoices(id).find((c) => c.value === "summary")?.reason).toBeNull();
    }
  });

  it("opens on a selection it offers and can pick", () => {
    for (const { id } of PROVIDERS) {
      const opened = contextChoices(id).find((c) => c.value === defaultContext(id));
      expect(opened?.reason).toBeNull();
    }
    expect(CODE_CHOICES.map((c) => c.value)).toContain(DEFAULT_CODE);
  });
});
