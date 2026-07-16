import { describe, expect, it } from "vitest";
import { buildCatalog, capPerGroup, dedupeBest } from "@/data/modelCatalog/build";
import { indexModelsDev } from "@/data/modelCatalog/modelsDev";
import { lookupModel, lookupModelInList, modelIdCandidates } from "@/data/modelCatalog/normalize";
import type { AgentModels, ModelMeta, SlimCatalog } from "@/data/modelCatalog/types";

const API = {
  anthropic: {
    models: {
      "claude-opus-4-8": {
        name: "Claude Opus 4.8",
        reasoning: true,
        release_date: "2026-05-28",
        limit: { context: 1_000_000 },
      },
      "claude-opus-4-7": {
        name: "Claude Opus 4.7",
        reasoning: true,
        release_date: "2026-04-16",
        limit: { context: 1_000_000 },
      },
      "claude-opus-4-6": {
        name: "Claude Opus 4.6",
        reasoning: true,
        release_date: "2026-02-05",
        limit: { context: 1_000_000 },
      },
      "claude-opus-4-5": {
        name: "Claude Opus 4.5 (latest)",
        reasoning: true,
        release_date: "2025-11-24",
        limit: { context: 200_000 },
      },
      "claude-opus-4-5-20251101": {
        name: "Claude Opus 4.5",
        reasoning: true,
        release_date: "2025-11-01",
        limit: { context: 200_000 },
      },
      "claude-sonnet-4-6": {
        name: "Claude Sonnet 4.6",
        reasoning: true,
        release_date: "2026-02-17",
        limit: { context: 200_000 },
      },
      "claude-sonnet-4-5": {
        name: "Claude Sonnet 4.5 (latest)",
        reasoning: true,
        release_date: "2025-09-29",
        limit: { context: 200_000 },
      },
      "claude-sonnet-4-5-20250929": {
        name: "Claude Sonnet 4.5",
        reasoning: true,
        release_date: "2025-09-29",
        limit: { context: 200_000 },
      },
      "claude-haiku-4-5": {
        name: "Claude Haiku 4.5 (latest)",
        reasoning: true,
        release_date: "2025-10-15",
        limit: { context: 200_000 },
      },
      "claude-haiku-4-5-20251001": {
        name: "Claude Haiku 4.5",
        reasoning: true,
        release_date: "2025-10-15",
        limit: { context: 200_000 },
      },
      "claude-3-5-haiku-20241022": {
        name: "Claude 3.5 Haiku",
        reasoning: false,
        release_date: "2024-10-22",
        limit: { context: 200_000 },
      },
    },
  },
  openai: {
    models: {
      "gpt-5.3-codex": {
        name: "Codex 5.3",
        reasoning: true,
        release_date: "2026-01-05",
        limit: { context: 400_000 },
      },
      "gpt-5.5": {
        name: "GPT-5.5",
        reasoning: true,
        release_date: "2025-12-11",
        limit: { context: 400_000 },
      },
      "gpt-5.4": {
        name: "GPT-5.4",
        reasoning: true,
        release_date: "2025-11-15",
        limit: { context: 400_000 },
      },
      "gpt-5.2-codex": {
        name: "Codex 5.2",
        reasoning: true,
        release_date: "2025-10-20",
        limit: { context: 400_000 },
      },
    },
  },
  google: {
    models: {
      "gemini-3.5-pro": {
        name: "Gemini 3.5 Pro",
        reasoning: true,
        release_date: "2026-05-20",
        limit: { context: 1_000_000 },
      },
      "gemini-3.5-flash": {
        name: "Gemini 3.5 Flash",
        reasoning: true,
        release_date: "2026-05-19",
        limit: { context: 1_000_000 },
      },
      "gemini-3.1-pro": {
        name: "Gemini 3.1 Pro",
        reasoning: true,
        release_date: "2025-12-04",
        limit: { context: 1_000_000 },
      },
      "gemini-3.1-pro-preview": {
        name: "Gemini 3.1 Pro Preview",
        reasoning: true,
        release_date: "2025-12-04",
        limit: { context: 1_000_000 },
      },
      "gemini-3.0-pro": {
        name: "Gemini 3.0 Pro",
        reasoning: true,
        release_date: "2025-11-10",
        limit: { context: 1_000_000 },
      },
      "gemini-3.0-flash": {
        name: "Gemini 3.0 Flash",
        reasoning: true,
        release_date: "2025-11-12",
        limit: { context: 1_000_000 },
      },
      "gemini-2.5-flash-preview-tts": {
        name: "Gemini 2.5 Flash Preview TTS",
        reasoning: true,
        release_date: "2025-05-01",
        limit: { context: 32_000 },
      },
      "gemini-2.5-pro": {
        name: "Gemini 2.5 Pro",
        reasoning: true,
        release_date: "2025-06-17",
        limit: { context: 1_000_000 },
      },
      "gemini-2.5-flash": {
        name: "Gemini 2.5 Flash",
        reasoning: true,
        release_date: "2025-06-17",
        limit: { context: 1_000_000 },
      },
      "gemini-2.0-pro": {
        name: "Gemini 2.0 Pro",
        reasoning: true,
        release_date: "2025-02-05",
        limit: { context: 1_000_000 },
      },
      "gemini-1.5-pro": {
        name: "Gemini 1.5 Pro",
        reasoning: true,
        release_date: "2024-05-14",
        limit: { context: 1_000_000 },
      },
      "nano-banana": {
        name: "Nano Banana",
        reasoning: false,
        release_date: "2025-12-20",
        limit: { context: 32_000 },
      },
      "gemini-3-pro-image-preview": {
        name: "Nano Banana Pro",
        reasoning: true,
        release_date: "2025-11-20",
        limit: { context: 32_000 },
      },
      "gemini-3.1-flash-lite": {
        name: "Gemini 3.1 Flash Lite",
        reasoning: true,
        release_date: "2026-05-07",
        limit: { context: 1_000_000 },
      },
    },
  },
  xai: {
    models: {
      "grok-4.3": {
        name: "Grok 4.3",
        reasoning: true,
        release_date: "2025-12-02",
        limit: { context: 1_000_000 },
      },
      "grok-4.2": {
        name: "Grok 4.2",
        reasoning: true,
        release_date: "2025-11-05",
        limit: { context: 1_000_000 },
      },
      "grok-4.1": {
        name: "Grok 4.1",
        reasoning: true,
        release_date: "2025-09-01",
        limit: { context: 1_000_000 },
      },
    },
  },
  kimi: {
    models: {
      "kimi-k2.5": {
        name: "Kimi K2.5",
        reasoning: true,
        release_date: "2025-12-01",
        limit: { context: 256_000 },
      },
      "kimi-k2": {
        name: "Kimi K2",
        reasoning: true,
        release_date: "2025-08-15",
        limit: { context: 256_000 },
      },
      "kimi-k1": {
        name: "Kimi K1",
        reasoning: false,
        release_date: "2025-04-10",
        limit: { context: 128_000 },
      },
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
    expect(idx.byId["gpt-5.5"]).toEqual({
      id: "gpt-5.5",
      name: "GPT-5.5",
      contextWindow: 400_000,
      reasoning: true,
      releaseDate: "2025-12-11",
    });
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
    expect(cat.byAgent.claude.map((m) => m.name).some((name) => name.includes("(latest)"))).toBe(
      false,
    );
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

  it("passes through a model's reasoning levels and default (CLI-only metadata)", () => {
    const agents: AgentModels[] = [
      {
        agent: "codex",
        models: [
          {
            id: "gpt-5.5",
            reasoning: true,
            reasoningLevels: ["low", "medium", "high", "xhigh", "max", "ultra"],
            defaultReasoning: "low",
          },
        ],
      },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["gpt-5.5"].reasoningLevels).toEqual([
      "low",
      "medium",
      "high",
      "xhigh",
      "max",
      "ultra",
    ]);
    expect(cat.byId["gpt-5.5"].defaultReasoning).toBe("low");
  });

  it("omits reasoning levels when the CLI reports none", () => {
    const agents: AgentModels[] = [
      { agent: "codex", models: [{ id: "gpt-5.5", reasoning: true }] },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["gpt-5.5"].reasoningLevels).toBeUndefined();
    expect(cat.byId["gpt-5.5"].defaultReasoning).toBeUndefined();
  });

  it("offers no selectable models for Antigravity (agy ignores model selection)", () => {
    // Discovery contributes no hint and no models for antigravity, so the
    // picker shows it as a fixed-model agent rather than offering Gemini ids
    // agy can't honor.
    const agents: AgentModels[] = [{ agent: "antigravity", models: [] }];
    const cat = buildCatalog(agents, idx);
    expect(cat.byAgent.antigravity).toEqual([]);
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
    expect(cat.byAgent.cursor.map((m) => m.name).some((name) => name.includes("(default)"))).toBe(
      false,
    );
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

  it("curates Pi like Claude: group by family, newest first, capped per family", () => {
    const agents: AgentModels[] = [
      {
        agent: "pi",
        models: [
          { id: "claude-3-5-haiku-20241022" },
          { id: "claude-3-7-sonnet-20250219" },
          { id: "claude-3-opus-20240229" },
          { id: "claude-haiku-4-5" },
          { id: "claude-haiku-4-5-20251001" },
          { id: "claude-opus-4-0" },
          { id: "claude-opus-4-1" },
          { id: "claude-opus-4-5" },
          { id: "claude-opus-4-5-20251101" },
          { id: "claude-opus-4-6" },
          { id: "claude-opus-4-7" },
          { id: "claude-opus-4-8" },
          { id: "claude-sonnet-4-0" },
          { id: "claude-sonnet-4-5" },
          { id: "claude-sonnet-4-5-20250929" },
          { id: "claude-sonnet-4-6" },
        ],
      },
    ];
    const cat = buildCatalog(agents, idx);

    expect(cat.byAgent.pi.map((m) => m.id)).toEqual([
      "claude-opus-4-8",
      "claude-opus-4-7",
      "claude-opus-4-6",
      "claude-sonnet-4-6",
      "claude-sonnet-4-5",
      "claude-haiku-4-5",
    ]);
    expect(cat.byAgent.pi.map((m) => m.id)).not.toContain("claude-3-5-haiku-20241022");
    expect(cat.byAgent.pi.map((m) => m.id)).not.toContain("claude-3-7-sonnet-20250219");
    expect(cat.byAgent.pi.map((m) => m.id)).not.toContain("claude-opus-4-1");
    expect(cat.byAgent.pi.map((m) => m.name).some((name) => name.includes("(latest)"))).toBe(false);
  });

  it("keeps Pi models from other providers after the Claude families", () => {
    const agents: AgentModels[] = [
      {
        agent: "pi",
        models: [{ id: "claude-opus-4-8" }, { id: "gpt-5.5" }, { id: "gemini-3.5-pro" }],
      },
    ];
    const cat = buildCatalog(agents, idx);

    expect(cat.byAgent.pi.map((m) => m.id)).toEqual([
      "claude-opus-4-8",
      "gemini-3.5-pro",
      "gpt-5.5",
    ]);
  });

  it("falls back to CLI metadata for ids models.dev doesn't know", () => {
    const agents: AgentModels[] = [
      {
        agent: "opencode",
        models: [{ id: "big-pickle", name: "Big Pickle", contextWindow: 32_000 }],
      },
    ];
    const cat = buildCatalog(agents, idx);
    expect(cat.byId["big-pickle"]).toEqual({
      id: "big-pickle",
      name: "Big Pickle",
      contextWindow: 32_000,
      reasoning: false,
    });
  });
});

describe("buildCatalog — data-driven Claude families", () => {
  // Faithful to models.dev: every model carries a `family` (e.g. "claude-fable"),
  // including a family the name-based classifier would never recognize.
  const FAMILY_API = {
    anthropic: {
      models: {
        "claude-opus-4-8": {
          name: "Claude Opus 4.8",
          family: "claude-opus",
          reasoning: true,
          release_date: "2026-05-28",
          limit: { context: 1_000_000 },
        },
        "claude-fable-5": {
          name: "Claude Fable 5",
          family: "claude-fable",
          reasoning: true,
          release_date: "2026-06-09",
          limit: { context: 1_000_000 },
        },
      },
    },
    openai: {
      models: {
        "gpt-5.5": {
          name: "GPT-5.5",
          family: "gpt-5",
          reasoning: true,
          release_date: "2025-12-11",
          limit: { context: 400_000 },
        },
      },
    },
  };
  const idx = indexModelsDev(FAMILY_API as never);

  it("surfaces a new Claude family (Fable) via the models.dev family field", () => {
    const agents: AgentModels[] = [{ agent: "claude", providerHint: "anthropic", models: [] }];
    const ids = buildCatalog(agents, idx).byAgent.claude.map((m) => m.id);

    // Fable's name/id contain no opus/sonnet/haiku, so it's recognized only by
    // the family field — and a new flagship family sorts ahead, not buried.
    expect(ids).toContain("claude-fable-5");
    expect(ids[0]).toBe("claude-fable-5");
  });

  it("does not misclassify a non-Claude family as Claude in Pi curation", () => {
    // gpt-5.5 has a family field too; it must stay after the Claude models,
    // not get grouped as a Claude family.
    const agents: AgentModels[] = [
      { agent: "pi", models: [{ id: "claude-opus-4-8" }, { id: "gpt-5.5" }] },
    ];
    expect(buildCatalog(agents, idx).byAgent.pi.map((m) => m.id)).toEqual([
      "claude-opus-4-8",
      "gpt-5.5",
    ]);
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
    "claude-opus-4": {
      id: "claude-opus-4",
      name: "Claude Opus 4",
      contextWindow: 200_000,
      reasoning: true,
    },
  };

  it("matches after stripping a date suffix and provider prefix", () => {
    expect(lookupModel(catalog, "anthropic/claude-opus-4-20250514")?.name).toBe("Claude Opus 4");
  });

  it("returns undefined for unknown or empty ids", () => {
    expect(lookupModel(catalog, "made-up")).toBeUndefined();
    expect(lookupModel(catalog, undefined)).toBeUndefined();
  });
});

describe("lookupModelInList", () => {
  // Two providers report the same id; only codex carries reasoning levels. The
  // global byId view keeps whichever was discovered first, so a provider-scoped
  // lookup must find codex's richer entry when codex is the selected provider.
  const codexEntry: ModelMeta = {
    id: "gpt-5.6-sol",
    name: "GPT-5.6-Sol",
    contextWindow: 272_000,
    reasoning: true,
    reasoningLevels: ["low", "high", "max"],
    defaultReasoning: "low",
  };
  const opencodeEntry: ModelMeta = {
    id: "gpt-5.6-sol",
    name: "GPT-5.6-Sol",
    contextWindow: 272_000,
    reasoning: true,
  };

  it("finds the provider's own entry, including per-model reasoning metadata", () => {
    const hit = lookupModelInList([codexEntry], "gpt-5.6-sol");
    expect(hit?.reasoningLevels).toEqual(["low", "high", "max"]);
    expect(hit?.defaultReasoning).toBe("low");
  });

  it("matches id candidates (provider prefix / date suffix)", () => {
    expect(lookupModelInList([codexEntry], "openai/gpt-5.6-sol")?.name).toBe("GPT-5.6-Sol");
  });

  it("does not return another provider's leaner entry", () => {
    // opencode's entry lacks reasoning levels; scoping to it must not surface
    // codex's, so callers can fall back to their default.
    expect(lookupModelInList([opencodeEntry], "gpt-5.6-sol")?.reasoningLevels).toBeUndefined();
  });

  it("returns undefined for empty/unknown input", () => {
    expect(lookupModelInList(undefined, "gpt-5.6-sol")).toBeUndefined();
    expect(lookupModelInList([codexEntry], undefined)).toBeUndefined();
    expect(lookupModelInList([codexEntry], "made-up")).toBeUndefined();
  });
});

const model = (id: string, name = id): ModelMeta => ({
  id,
  name,
  contextWindow: 0,
  reasoning: false,
});

describe("dedupeBest", () => {
  // Group every model by its leading token: "a-1" and "a-2" share group "a".
  const byPrefix = (m: ModelMeta) => m.id.split("-")[0];
  // Higher trailing number wins.
  const preferHigher = (a: ModelMeta, b: ModelMeta) =>
    Number(a.id.split("-")[1]) > Number(b.id.split("-")[1]);

  it("keeps the preferred representative at the group's first-seen position", () => {
    // 'a' first appears at index 0; its best (a-3) must land there, not at a-3's index.
    const out = dedupeBest(
      [model("a-1"), model("b-1"), model("a-3"), model("a-2")],
      byPrefix,
      preferHigher,
    );
    expect(out.map((m) => m.id)).toEqual(["a-3", "b-1"]);
  });

  it("keeps the current representative when prefer rejects the candidate", () => {
    const out = dedupeBest([model("a-3"), model("a-1")], byPrefix, preferHigher);
    expect(out.map((m) => m.id)).toEqual(["a-3"]);
  });

  it("drops models whose key is null", () => {
    const keyFn = (m: ModelMeta) => (m.id.startsWith("keep") ? "k" : null);
    const out = dedupeBest([model("drop-1"), model("keep-1"), model("drop-2")], keyFn, () => false);
    expect(out.map((m) => m.id)).toEqual(["keep-1"]);
  });
});

describe("capPerGroup", () => {
  const byPrefix = (m: ModelMeta) => m.id.split("-")[0] as "a" | "b";

  it("keeps at most caps[group] per group, in input order", () => {
    const models = [model("a-1"), model("a-2"), model("b-1"), model("a-3"), model("b-2")];
    const out = capPerGroup(models, byPrefix, { a: 2, b: 1 });
    expect(out.map((m) => m.id)).toEqual(["a-1", "a-2", "b-1"]);
  });

  it("drops models whose group is null", () => {
    const keyFn = (m: ModelMeta) => (m.id.startsWith("keep") ? ("k" as const) : null);
    const out = capPerGroup([model("keep-1"), model("skip-1")], keyFn, { k: 5 });
    expect(out.map((m) => m.id)).toEqual(["keep-1"]);
  });

  it("drops every model in a group capped at zero", () => {
    const out = capPerGroup([model("a-1"), model("b-1")], byPrefix, { a: 0, b: 1 });
    expect(out.map((m) => m.id)).toEqual(["b-1"]);
  });
});
