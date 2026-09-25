import { describe, expect, it } from "vitest";
import { agentIdentityTip, dropSharedBrand, modelDisplayName } from "@/data/modelCatalog/display";
import type { SlimCatalog } from "@/data/modelCatalog/types";

const CATALOG: SlimCatalog = {
  "claude-fable-5-1": {
    id: "claude-fable-5-1",
    name: "Claude Fable 5.1",
    contextWindow: 1_000_000,
    reasoning: true,
  },
  "gpt-5-codex": { id: "gpt-5-codex", name: "GPT-5 Codex", contextWindow: 0, reasoning: true },
};

describe("modelDisplayName", () => {
  it("uses the catalog name, matching through prefixes and tags", () => {
    expect(modelDisplayName(CATALOG, "claude-fable-5-1")).toBe("Claude Fable 5.1");
    expect(modelDisplayName(CATALOG, "anthropic/claude-fable-5-1[1m]")).toBe("Claude Fable 5.1");
  });

  it("falls back to the bare id for an unknown model", () => {
    expect(modelDisplayName(CATALOG, "openai/gpt-9-preview[1m]")).toBe("gpt-9-preview");
  });

  it("is null without an id", () => {
    expect(modelDisplayName(CATALOG, null)).toBeNull();
    expect(modelDisplayName(CATALOG, "")).toBeNull();
  });
});

describe("dropSharedBrand", () => {
  it("drops a brand word the provider label already carries", () => {
    expect(dropSharedBrand("Claude Fable 5.1", "Claude Code")).toBe("Fable 5.1");
  });

  it("keeps the name when the brands differ", () => {
    expect(dropSharedBrand("Claude Opus 4.5", "Cursor Agent")).toBe("Claude Opus 4.5");
    expect(dropSharedBrand("GPT-5 Codex", "Codex")).toBe("GPT-5 Codex");
  });

  it("never returns an empty name", () => {
    expect(dropSharedBrand("Claude", "Claude Code")).toBe("Claude");
  });
});

describe("agentIdentityTip", () => {
  it("reads provider · model · effort", () => {
    expect(
      agentIdentityTip({
        providerLabel: "Claude Code",
        catalog: CATALOG,
        model: "claude-fable-5-1",
        effort: "high",
      }),
    ).toBe("Claude Code · Fable 5.1 · High effort");
  });

  it("labels effort the way the thinking picker does", () => {
    expect(
      agentIdentityTip({
        providerLabel: "Claude Code",
        catalog: CATALOG,
        model: "claude-fable-5-1",
        effort: "xhigh",
      }),
    ).toBe("Claude Code · Fable 5.1 · xHigh effort");
  });

  it("falls back to the transcript-reported model for a default-model session", () => {
    expect(
      agentIdentityTip({
        providerLabel: "Codex",
        catalog: CATALOG,
        model: null,
        liveModel: "gpt-5-codex",
      }),
    ).toBe("Codex · GPT-5 Codex");
  });

  it("is just the provider when nothing else is known", () => {
    expect(agentIdentityTip({ providerLabel: "Claude Code", catalog: CATALOG })).toBe(
      "Claude Code",
    );
  });

  it("leads with the custom agent's name", () => {
    expect(
      agentIdentityTip({
        providerLabel: "Claude Code",
        customAgentName: "Reviewer",
        catalog: CATALOG,
        model: "claude-fable-5-1",
      }),
    ).toBe("Reviewer · Claude Code · Fable 5.1");
  });
});
