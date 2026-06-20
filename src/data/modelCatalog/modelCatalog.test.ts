import { describe, it, expect } from "vitest";
import { indexModelsDev } from "./modelsDev";
import { buildCatalog } from "./build";
import { lookupModel, modelIdCandidates } from "./normalize";
import type { AgentModels, SlimCatalog } from "./types";

const API = {
  anthropic: {
    models: {
      "claude-opus-4-8": { name: "Claude Opus 4.8", reasoning: true, release_date: "2026-05-28", limit: { context: 1_000_000 } },
      "claude-opus-4-7": { name: "Claude Opus 4.7", reasoning: true, release_date: "2026-04-16", limit: { context: 1_000_000 } },
      "claude-opus-4-6": { name: "Claude Opus 4.6", reasoning: true, release_date: "2026-02-05", limit: { context: 1_000_000 } },
      "claude-opus-4-5": { name: "Claude Opus 4.5 (latest)", reasoning: true, release_date: "2025-11-24", limit: { context: 200_000 } },
      "claude-opus-4-5-20251101": { name: "Claude Opus 4.5", reasoning: true, release_date: "2025-11-01", limit: { context: 200_000 } },
      "claude-sonnet-4-6": { name: "Claude Sonnet 4.6", reasoning: true, release_date: "2026-02-17", limit: { context: 200_000 } },
      "claude-sonnet-4-5": { name: "Claude Sonnet 4.5 (latest)", reasoning: true, release_date: "2025-09-29", limit: { context: 200_000 } },
      "claude-sonnet-4-5-20250929": { name: "Claude Sonnet 4.5", reasoning: true, release_date: "2025-09-29", limit: { context: 200_000 } },
      "claude-haiku-4-5": { name: "Claude Haiku 4.5 (latest)", reasoning: true, release_date: "2025-10-15", limit: { context: 200_000 } },
      "claude-haiku-4-5-20251001": { name: "Claude Haiku 4.5", reasoning: true, release_date: "2025-10-15", limit: { context: 200_000 } },
      "claude-3-5-haiku-20241022": { name: "Claude 3.5 Haiku", reasoning: false, release_date: "2024-10-22", limit: { context: 200_000 } },
    },
  },
  openai: {
    models: {
      "gpt-5.3-codex": { name: "Codex 5.3", reasoning: true, release_date: "2026-01-05", limit: { context: 400_000 } },
      "gpt-5.5": { name: "GPT-5.5", reasoning: true, release_date: "2025-12-11", limit: { context: 400_000 } },
      "gpt-5.4": { name: "GPT-5.4", reasoning: true, release_date: "2025-11-15", limit: { context: 400_000 } },
      "gpt-5.2-codex": { name: "Codex 5.2", reasoning: true, release_date: "2025-10-20", limit: { context: 400_000 } },
    },
  },
  google: {
    models: {
      "gemini-3.5-pro": { name: "Gemini 3.5 Pro", reasoning: true, release_date: "2026-05-20", limit: { context: 1_000_000 } },
      "gemini-3.5-flash": { name: "Gemini 3.5 Flash", reasoning: true, release_date: "2026-05-19", limit: { context: 1_000_000 } },
      "gemini-3.1-pro": { name: "Gemini 3.1 Pro", reasoning: true, release_date: "2025-12-04", limit: { context: 1_000_000 } },
      "gemini-3.1-pro-preview": { name: "Gemini 3.1 Pro Preview", reasoning: true, release_date: "2025-12-04", limit: { context: 1_000_000 } },
      "gemini-3.0-pro": { name: "Gemini 3.0 Pro", reasoning: true, release_date: "2025-11-10", limit: { context: 1_000_000 } },
      "gemini-3.0-flash": { name: "Gemini 3.0 Flash", reasoning: true, release_date: "2025-11-12", limit: { context: 1_000_000 } },
      "gemini-2.5-flash-preview-tts": { name: "Gemini 2.5 Flash Preview TTS", reasoning: true, release_date: "2025-05-01", limit: { context: 32_000 } },
      "gemini-2.5-pro": { name: "Gemini 2.5 Pro", reasoning: true, release_date: "2025-06-17", limit: { context: 1_000_000 } },
      "gemini-2.5-flash": { name: "Gemini 2.5 Flash", reasoning: true, release_date: "2025-06-17", limit: { context: 1_000_000 } },
      "gemini-2.0-pro": { name: "Gemini 2.0 Pro", reasoning: true, release_date: "2025-02-05", limit: { context: 1_000_000 } },
      "gemini-1.5-pro": { name: "Gemini 1.5 Pro", reasoning: true, release_date: "2024-05-14", limit: { context: 1_000_000 } },
      "nano-banana": { name: "Nano Banana", reasoning: false, release_date: "2025-12-20", limit: { context: 32_000 } },
      "gemini-3-pro-image-preview": { name: "Nano Banana Pro", reasoning: true, release_date: "2025-11-20", limit: { context: 32_000 } },
      "gemini-3.1-flash-lite": { name: "Gemini 3.1 Flash Lite", reasoning: true, release_date: "2026-05-07", limit: { context: 1_000_000 } },
    },
  },
  xai: {
    models: {
      "grok-4.3": { name: "Grok 4.3", reasoning: true, release_date: "2025-12-02", limit: { context: 1_000_000 } },
      "grok-4.2": { name: "Grok 4.2", reasoning: true, release_date: "2025-11-05", limit: { context: 1_000_000 } },
      "grok-4.1": { name: "Grok 4.1", reasoning: true, release_date: "2025-09-01", limit: { context: 1_000_000 } },
    },
  },
  kimi: {
    models: {
      "kimi-k2.5": { name: "Kimi K2.5", reasoning: true, release_date: "2025-12-01", limit: { context: 256_000 } },
      "kimi-k2": { name: "Kimi K2", reasoning: true, release_date: "2025-08-15", limit: { context: 256_000 } },
      "kimi-k1": { name: "Kimi K1", reasoning: false, release_date: "2025-04-10", limit: { context: 128_000 } },
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
    expect(idx.byId["gpt-5.5"]).toEqual({ id: "gpt-5.5", name: "GPT-5.5", contextWindow: 400_000, reasoning: true, releaseDate: "2025-12-11" });
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
    expect(cat.byAgent.claude).toHaveLength(6);
    expect(cat.byAgent.claude.map((m) => m.id)).toEqual([
      "claude-opus-4-8",
      "claude-opus-4-7",
      "claude-opus-4-6",
      "claude-sonnet-4-6",
      "claude-sonnet-4-5",
      "claude-haiku-4-5",
    ]);
    expect(cat.byAgent.claude.map((m) => m.id)).not.toContain("claude-opus-4-5-20251101");
    expect(cat.byAgent.claude.map((m) => m.id)).not.toContain("claude-3-5-haiku-20241022");
    expect(cat.byAgent.claude.map((m) => m.name)).toContain("Claude Sonnet 4.5");
    expect(cat.byAgent.claude.map((m) => m.name).some((name) => name.includes("(latest)"))).toBe(false);
    // The 1M variant the transcript reports resolves through the by-id map.
    expect(lookupModel(cat.byId, "claude-opus-4-8[1m]")?.contextWindow).toBe(1_000_000);
  });

  it("enriches discovered ids with models.dev metadata", () => {
    const agents: AgentModels[] = [
      { agent: "codex", models: [{ id: "gpt-5.5", reasoning: true }] },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["gpt-5.5"].contextWindow).toBe(400_000);
    expect(cat.byId["gpt-5.5"].releaseDate).toBe("2025-12-11");
    expect(cat.byAgent.codex).toHaveLength(1);
  });

  it("curates Antigravity to a short Gemini Pro and Flash flagship list", () => {
    const agents: AgentModels[] = [{ agent: "antigravity", providerHint: "google", models: [] }];
    const cat = buildCatalog(agents, idx);

    expect(cat.byAgent.antigravity.map((m) => m.id)).toEqual([
      "gemini-3.5-pro",
      "gemini-3.5-flash",
      "gemini-3.1-pro",
      "gemini-3.0-flash",
    ]);
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("nano-banana");
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("gemini-3-pro-image-preview");
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("gemini-3.1-flash-lite");
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("gemini-2.5-flash-preview-tts");
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("gemini-3.1-pro-preview");
    expect(cat.byAgent.antigravity.map((m) => m.id)).not.toContain("gemini-1.5-pro");
  });

  it("orders discovered models by release date descending, unknown dates last", () => {
    const agents: AgentModels[] = [
      {
        agent: "codex",
        models: [
          { id: "unknown-local", name: "Unknown Local" },
          { id: "claude-3-5-haiku-20241022" },
          { id: "claude-opus-4-7" },
          { id: "gpt-5.5" },
        ],
      },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byAgent.codex.map((m) => m.id)).toEqual([
      "claude-opus-4-7",
      "gpt-5.5",
      "claude-3-5-haiku-20241022",
      "unknown-local",
    ]);
  });

  it("curates Cursor to recent Composer models and a few flagship models per provider", () => {
    const agents: AgentModels[] = [
      {
        agent: "cursor",
        models: [
          { id: "composer-2.5-fast", name: "Composer 2.5 Fast (default)" },
          { id: "composer-2.5", name: "Composer 2.5" },
          { id: "composer-2.4", name: "Composer 2.4" },
          { id: "composer-2.3", name: "Composer 2.3" },
          { id: "composer-2.2", name: "Composer 2.2" },
          { id: "gpt-5.3-codex-low", name: "Codex 5.3 Low" },
          { id: "gpt-5.3-codex", name: "Codex 5.3" },
          { id: "claude-opus-4-8-thinking-high", name: "Opus 4.8 1M Thinking High" },
          { id: "claude-opus-4-8-thinking", name: "Opus 4.8 1M Thinking" },
          { id: "claude-opus-4-7-thinking", name: "Opus 4.7 1M Thinking" },
          { id: "claude-4.6-sonnet-medium", name: "Sonnet 4.6 1M" },
          { id: "claude-haiku-4-5", name: "Haiku 4.5" },
          { id: "gpt-5.5-high", name: "GPT-5.5 1M High" },
          { id: "gpt-5.5-medium", name: "GPT-5.5 1M" },
          { id: "gpt-5.4-medium", name: "GPT-5.4 1M" },
          { id: "gpt-5.2-codex", name: "Codex 5.2" },
          { id: "gemini-3.1-pro", name: "Gemini 3.1 Pro" },
          { id: "gemini-3.0-pro", name: "Gemini 3.0 Pro" },
          { id: "gemini-2.5-pro", name: "Gemini 2.5 Pro" },
          { id: "gemini-2.0-pro", name: "Gemini 2.0 Pro" },
          { id: "grok-4.3", name: "Grok 4.3 1M" },
          { id: "grok-4.2", name: "Grok 4.2 1M" },
          { id: "grok-4.1", name: "Grok 4.1 1M" },
          { id: "kimi-k2.5", name: "Kimi K2.5" },
          { id: "kimi-k2", name: "Kimi K2" },
          { id: "kimi-k1", name: "Kimi K1" },
        ],
      },
    ];
    const cat = buildCatalog(agents, idx);

    expect(cat.byAgent.cursor.map((m) => m.id)).toEqual([
      "composer-2.5-fast",
      "composer-2.5",
      "composer-2.4",
      "composer-2.3",
      "claude-opus-4-8-thinking",
      "claude-opus-4-7-thinking",
      "claude-4.6-sonnet-medium",
      "gpt-5.3-codex",
      "gpt-5.5-medium",
      "gpt-5.4-medium",
      "gemini-3.1-pro",
      "gemini-3.0-pro",
      "gemini-2.5-pro",
      "grok-4.3",
      "grok-4.2",
      "kimi-k2.5",
      "kimi-k2",
    ]);
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("composer-2.2");
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("claude-haiku-4-5");
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("gpt-5.2-codex");
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("gemini-2.0-pro");
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("grok-4.1");
    expect(cat.byAgent.cursor.map((m) => m.id)).not.toContain("kimi-k1");
    expect(cat.byAgent.cursor[0].name).toBe("Composer 2.5 Fast");
    expect(cat.byAgent.cursor.map((m) => m.name).some((name) => name.includes("(default)"))).toBe(false);
    expect(cat.byId["gpt-5.5-medium"].releaseDate).toBe("2025-12-11");
    expect(cat.byId["claude-4.6-sonnet-medium"].releaseDate).toBe("2026-02-17");
  });

  it("groups Cursor Claude models by family after Composer", () => {
    const agents: AgentModels[] = [
      {
        agent: "cursor",
        models: [
          { id: "composer-2.5", name: "Composer 2.5" },
          { id: "claude-opus-4-8-thinking", name: "Opus 4.8 1M Thinking" },
          { id: "claude-haiku-4-5", name: "Haiku 4.5" },
          { id: "claude-sonnet-4-5", name: "Sonnet 4.5" },
          { id: "gpt-5.5-medium", name: "GPT-5.5 1M" },
        ],
      },
    ];
    const cat = buildCatalog(agents, idx);

    expect(cat.byAgent.cursor.map((m) => m.id)).toEqual([
      "composer-2.5",
      "claude-opus-4-8-thinking",
      "claude-sonnet-4-5",
      "claude-haiku-4-5",
      "gpt-5.5-medium",
    ]);
  });

  it("falls back to CLI metadata for ids models.dev doesn't know", () => {
    const agents: AgentModels[] = [
      { agent: "opencode", models: [{ id: "big-pickle", name: "Big Pickle", contextWindow: 32_000 }] },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["big-pickle"]).toEqual({ id: "big-pickle", name: "Big Pickle", contextWindow: 32_000, reasoning: false });
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
    "claude-opus-4": { id: "claude-opus-4", name: "Claude Opus 4", contextWindow: 200_000, reasoning: true },
  };

  it("matches after stripping a date suffix and provider prefix", () => {
    expect(lookupModel(catalog, "anthropic/claude-opus-4-20250514")?.name).toBe("Claude Opus 4");
  });

  it("returns undefined for unknown or empty ids", () => {
    expect(lookupModel(catalog, "made-up")).toBeUndefined();
    expect(lookupModel(catalog, undefined)).toBeUndefined();
  });
});
