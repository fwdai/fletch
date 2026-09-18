import { describe, expect, it } from "vitest";
import { buildCatalog } from "@/data/modelCatalog/build";
import { indexModelsDev } from "@/data/modelCatalog/modelsDev";
import { cacheSavingsUsd, modelCost, priceTokens } from "@/data/modelCatalog/pricing";
import type { SlimCatalog } from "@/data/modelCatalog/types";

// The `cost` block below is models.dev's real shape and real numbers: rates in
// USD per MILLION tokens, cache rates optional (openai prices cache writes as
// ordinary input and lists none).
const API = {
  anthropic: {
    models: {
      "claude-opus-5": {
        name: "Claude Opus 5",
        limit: { context: 1_000_000 },
        cost: { input: 5, output: 25, cache_read: 0.5, cache_write: 6.25 },
      },
    },
  },
  openai: {
    models: {
      "gpt-5.5": {
        name: "GPT-5.5",
        limit: { context: 400_000 },
        cost: { input: 1.25, output: 10, cache_read: 0.125 },
      },
      // A model models.dev lists without any rates at all.
      "gpt-5.5-secret": { name: "GPT-5.5 Secret", limit: { context: 400_000 } },
      // Priced, but with no cache rates of its own.
      "gpt-5.5-flat": {
        name: "GPT-5.5 Flat",
        limit: { context: 400_000 },
        cost: { input: 2, output: 6 },
      },
    },
  },
};

const catalog: SlimCatalog = indexModelsDev(API as never).byId;

const tokens = (t: Partial<Record<"input" | "output" | "cacheRead" | "cacheWrite", number>>) => ({
  input: t.input ?? 0,
  output: t.output ?? 0,
  cacheRead: t.cacheRead ?? 0,
  cacheWrite: t.cacheWrite ?? 0,
});

describe("cost from models.dev", () => {
  it("maps the api.json cost block, in USD per million tokens", () => {
    expect(catalog["claude-opus-5"].cost).toEqual({
      input: 5,
      output: 25,
      cacheRead: 0.5,
      cacheWrite: 6.25,
    });
  });

  it("falls back to the input rate for a cache rate the provider doesn't list", () => {
    // OpenAI charges cache writes at the ordinary input rate rather than a
    // premium, so an absent `cache_write` means "same as input", not "free".
    expect(catalog["gpt-5.5"].cost).toEqual({
      input: 1.25,
      output: 10,
      cacheRead: 0.125,
      cacheWrite: 1.25,
    });
  });

  it("leaves a model models.dev doesn't price unpriced, not free", () => {
    expect(catalog["gpt-5.5-secret"].cost).toBeUndefined();
    expect(modelCost(catalog, "gpt-5.5-secret")).toBeNull();
  });

  it("survives catalog assembly, for both discovery paths", () => {
    const index = indexModelsDev(API as never);
    const built = buildCatalog(
      [
        { agent: "claude", providerHint: "anthropic", models: [] },
        { agent: "codex", models: [{ id: "gpt-5.5" }] },
      ],
      index,
    );
    expect(built.byId["claude-opus-5"].cost?.output).toBe(25);
    expect(built.byId["gpt-5.5"].cost?.output).toBe(10);
  });
});

describe("priceTokens", () => {
  it("prices every category at its own rate", () => {
    const usd = priceTokens(
      catalog,
      "claude-opus-5",
      tokens({ input: 1_000_000, output: 1_000_000, cacheRead: 1_000_000, cacheWrite: 1_000_000 }),
    );
    expect(usd).toBeCloseTo(5 + 25 + 0.5 + 6.25, 10);
  });

  it("scales down to a real turn", () => {
    const usd = priceTokens(
      catalog,
      "claude-opus-5",
      tokens({ input: 2_000, output: 500, cacheRead: 100_000, cacheWrite: 20_000 }),
    );
    // 0.01 + 0.0125 + 0.05 + 0.125
    expect(usd).toBeCloseTo(0.1975, 10);
  });

  it("resolves a provider prefix and Claude's [1m] variant tag", () => {
    const t = tokens({ input: 1_000_000 });
    expect(priceTokens(catalog, "anthropic/claude-opus-5", t)).toBeCloseTo(5, 10);
    expect(priceTokens(catalog, "claude-opus-5[1m]", t)).toBeCloseTo(5, 10);
  });

  it("is null for ids that name no specific model", () => {
    const t = tokens({ input: 1_000 });
    // Claude's CLI-talking-to-itself placeholder, and the bare tier names an
    // agent reports when the user picked a family rather than a release.
    expect(priceTokens(catalog, "<synthetic>", t)).toBeNull();
    for (const bare of ["opus", "sonnet", "haiku", "fable"]) {
      expect(priceTokens(catalog, bare, t)).toBeNull();
    }
  });

  it("is null for an unknown, empty or absent model", () => {
    const t = tokens({ input: 1_000 });
    expect(priceTokens(catalog, "big-pickle", t)).toBeNull();
    expect(priceTokens(catalog, "", t)).toBeNull();
    expect(priceTokens(catalog, undefined, t)).toBeNull();
  });

  it("prices a zero-token spend at zero, not null", () => {
    expect(priceTokens(catalog, "claude-opus-5", tokens({}))).toBe(0);
  });
});

describe("cacheSavingsUsd", () => {
  it("is what the cache reads would have cost at the full input rate, less what they did", () => {
    const usd = cacheSavingsUsd(catalog, "claude-opus-5", tokens({ cacheRead: 1_000_000 }));
    expect(usd).toBeCloseTo(5 - 0.5, 10);
  });

  it("ignores every other category — only cache reads are discounted", () => {
    const usd = cacheSavingsUsd(
      catalog,
      "claude-opus-5",
      tokens({ input: 9_000_000, output: 9_000_000, cacheWrite: 9_000_000, cacheRead: 100_000 }),
    );
    expect(usd).toBeCloseTo(0.45, 10);
  });

  it("is zero when the provider charges cache reads at the input rate", () => {
    // An absent cache_read rate defaults to the input rate, so nothing is saved
    // — zero, and not a negative or an invented discount.
    expect(cacheSavingsUsd(catalog, "gpt-5.5-flat", tokens({ cacheRead: 1_000_000 }))).toBe(0);
    expect(cacheSavingsUsd(catalog, "claude-opus-5", tokens({ cacheRead: 0 }))).toBe(0);
  });

  it("is null for an unpriceable model", () => {
    expect(cacheSavingsUsd(catalog, "<synthetic>", tokens({ cacheRead: 1_000 }))).toBeNull();
    expect(cacheSavingsUsd(catalog, "big-pickle", tokens({ cacheRead: 1_000 }))).toBeNull();
  });
});
