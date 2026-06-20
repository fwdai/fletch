import { describe, it, expect } from "vitest";
import { indexModelsDev } from "./modelsDev";
import { buildCatalog } from "./build";
import { lookupModel, modelIdCandidates } from "./normalize";
import type { AgentModels, SlimCatalog } from "./types";

const API = {
  anthropic: {
    models: {
      "claude-opus-4-8": { name: "Claude Opus 4.8", reasoning: true, limit: { context: 1_000_000 } },
      "claude-3-5-haiku-20241022": { name: "Claude 3.5 Haiku", reasoning: false, limit: { context: 200_000 } },
    },
  },
  openai: {
    models: {
      "gpt-5.5": { name: "GPT-5.5", reasoning: true, limit: { context: 400_000 } },
    },
  },
  // A router re-listing a canonical id with a different window.
  opencode: {
    models: {
      "claude-opus-4-8": { name: "Opus via router", limit: { context: 123 } },
    },
  },
};

describe("indexModelsDev", () => {
  it("indexes metadata by id and ids by provider", () => {
    const idx = indexModelsDev(API as never);
    expect(idx.byId["gpt-5.5"]).toEqual({ name: "GPT-5.5", contextWindow: 400_000, reasoning: true });
    expect(idx.byProvider.anthropic).toContain("claude-opus-4-8");
  });

  it("lets the canonical provider win on id collisions", () => {
    const idx = indexModelsDev(API as never);
    expect(idx.byId["claude-opus-4-8"].contextWindow).toBe(1_000_000);
  });
});

describe("buildCatalog", () => {
  const idx = indexModelsDev(API as never);

  it("expands a provider hint (claude → all anthropic)", () => {
    const agents: AgentModels[] = [{ agent: "claude", providerHint: "anthropic", models: [] }];
    const cat = buildCatalog(agents, idx);
    expect(cat.byAgent.claude).toHaveLength(2);
    // The 1M variant the transcript reports resolves through the by-id map.
    expect(lookupModel(cat.byId, "claude-opus-4-8[1m]")?.contextWindow).toBe(1_000_000);
  });

  it("enriches discovered ids with models.dev metadata", () => {
    const agents: AgentModels[] = [
      { agent: "codex", models: [{ id: "gpt-5.5", reasoning: true }] },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["gpt-5.5"].contextWindow).toBe(400_000);
    expect(cat.byAgent.codex).toHaveLength(1);
  });

  it("falls back to CLI metadata for ids models.dev doesn't know", () => {
    const agents: AgentModels[] = [
      { agent: "opencode", models: [{ id: "big-pickle", name: "Big Pickle", contextWindow: 32_000 }] },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["big-pickle"]).toEqual({ name: "Big Pickle", contextWindow: 32_000, reasoning: false });
  });
});

describe("modelIdCandidates", () => {
  it("strips provider prefix and date suffix, in priority order", () => {
    expect(modelIdCandidates("anthropic/claude-opus-4-20250514")).toEqual([
      "anthropic/claude-opus-4-20250514",
      "claude-opus-4-20250514",
      "claude-opus-4",
    ]);
  });

  it("strips a trailing bracketed variant tag (Claude's [1m])", () => {
    expect(modelIdCandidates("claude-opus-4-8[1m]")).toContain("claude-opus-4-8");
  });
});

describe("lookupModel", () => {
  const catalog: SlimCatalog = {
    "claude-opus-4": { name: "Claude Opus 4", contextWindow: 200_000, reasoning: true },
  };

  it("matches after stripping a date suffix and provider prefix", () => {
    expect(lookupModel(catalog, "anthropic/claude-opus-4-20250514")?.name).toBe("Claude Opus 4");
  });

  it("returns undefined for unknown or empty ids", () => {
    expect(lookupModel(catalog, "made-up")).toBeUndefined();
    expect(lookupModel(catalog, undefined)).toBeUndefined();
  });
});
