import { describe, it, expect } from "vitest";
import { slimFullCatalog } from "./slim";
import { lookupModel, modelIdCandidates } from "./normalize";
import type { SlimCatalog } from "./types";

describe("slimFullCatalog", () => {
  const api = {
    anthropic: {
      models: {
        "claude-opus-4-8": {
          name: "Claude Opus 4.8",
          reasoning: true,
          limit: { context: 1_000_000, output: 64_000 },
        },
        "claude-3-5-haiku-20241022": {
          name: "Claude 3.5 Haiku",
          reasoning: false,
          limit: { context: 200_000 },
        },
      },
    },
    // A router that re-lists a canonical model id with a different window.
    opencode: {
      models: {
        "claude-opus-4-8": { name: "Opus via router", limit: { context: 123 } },
        "some-router-only": { name: "Router Only", limit: { context: 32_000 } },
      },
    },
  };

  it("flattens models by bare id and keeps only used fields", () => {
    const c = slimFullCatalog(api as never);
    expect(c["claude-3-5-haiku-20241022"]).toEqual({
      name: "Claude 3.5 Haiku",
      contextWindow: 200_000,
      reasoning: false,
    });
  });

  it("lets the canonical provider win on id collisions", () => {
    const c = slimFullCatalog(api as never);
    expect(c["claude-opus-4-8"].contextWindow).toBe(1_000_000);
    expect(c["claude-opus-4-8"].name).toBe("Claude Opus 4.8");
  });

  it("still includes router-only models", () => {
    const c = slimFullCatalog(api as never);
    expect(c["some-router-only"].contextWindow).toBe(32_000);
  });

  it("defaults reasoning to false and contextWindow to 0 when absent", () => {
    const c = slimFullCatalog({ x: { models: { m: { name: "M" } } } } as never);
    expect(c.m).toEqual({ name: "M", contextWindow: 0, reasoning: false });
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

  it("includes a lowercased fallback for mixed-case ids", () => {
    expect(modelIdCandidates("GPT-5")).toContain("gpt-5");
  });
});

describe("lookupModel", () => {
  const catalog: SlimCatalog = {
    "claude-opus-4": { name: "Claude Opus 4", contextWindow: 200_000, reasoning: true },
    "gpt-5.2-codex": { name: "GPT-5.2 Codex", contextWindow: 400_000, reasoning: true },
  };

  it("matches an exact bare id", () => {
    expect(lookupModel(catalog, "gpt-5.2-codex")?.contextWindow).toBe(400_000);
  });

  it("matches after stripping a date suffix", () => {
    expect(lookupModel(catalog, "claude-opus-4-20250514")?.name).toBe("Claude Opus 4");
  });

  it("matches after stripping a provider prefix", () => {
    expect(lookupModel(catalog, "anthropic/claude-opus-4")?.reasoning).toBe(true);
  });

  it("returns undefined for unknown or empty ids", () => {
    expect(lookupModel(catalog, "made-up-model")).toBeUndefined();
    expect(lookupModel(catalog, undefined)).toBeUndefined();
  });
});
