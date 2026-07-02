import { describe, expect, it } from "vitest";

import { parseNewDraftSelection } from "@/storage/preferences";

describe("parseNewDraftSelection", () => {
  it("reads a persisted provider and model", () => {
    expect(parseNewDraftSelection(JSON.stringify({ provider: "codex", model: "gpt-5.5" }))).toEqual(
      { provider: "codex", model: "gpt-5.5" },
    );
  });

  it("falls back to the default provider when the saved value is missing or invalid", () => {
    expect(parseNewDraftSelection(undefined)).toEqual({ provider: "claude" });
    expect(parseNewDraftSelection("not-json")).toEqual({ provider: "claude" });
    expect(parseNewDraftSelection(JSON.stringify({ provider: "   " }))).toEqual({
      provider: "claude",
    });
  });

  it("drops an empty model", () => {
    expect(parseNewDraftSelection(JSON.stringify({ provider: "cursor", model: "   " }))).toEqual({
      provider: "cursor",
    });
  });

  it("reads a persisted custom agent id", () => {
    expect(
      parseNewDraftSelection(
        JSON.stringify({ provider: "claude", model: "opus", customAgentId: "ca-1" }),
      ),
    ).toEqual({ provider: "claude", model: "opus", customAgentId: "ca-1" });
  });

  it("drops a blank custom agent id", () => {
    expect(
      parseNewDraftSelection(JSON.stringify({ provider: "claude", customAgentId: "  " })),
    ).toEqual({ provider: "claude" });
  });
});
