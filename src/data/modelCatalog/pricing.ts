// What a model charges, and what a pile of tokens costs at those rates.
//
// models.dev already carries a `cost` block per model and the catalog already
// fetches it, so pricing is metadata lookup rather than a table Fletch has to
// maintain: a new model is priceable the hour models.dev lists it.
//
// These are LIST API rates. A Max/Pro/Team subscription is not billed this way
// — a priced total is "what these tokens would have cost on the API", which is
// the only honest number available from a transcript.

import type { TokenCounts } from "@/adapters/usage";
import { lookupModel } from "./normalize";
import type { SlimCatalog } from "./types";

/** USD per million tokens, from models.dev `cost`. Missing cache rates fall
 *  back to the input rate — a provider that doesn't price caching separately
 *  charges cache traffic as ordinary input. */
export interface ModelCost {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** models.dev rates are per million tokens. */
const PER_MILLION = 1_000_000;

/** Ids that name no specific model, so no rate can apply to them. Claude's
 *  `<synthetic>` is the CLI talking to itself; a bare family name is what an
 *  agent reports when the user picked a tier rather than a release, and the
 *  tier spans models with different rates — guessing one would invent a
 *  number. Both resolve to null, which callers render as "unpriced". */
const UNPRICEABLE = new Set(["<synthetic>", "opus", "sonnet", "haiku", "fable"]);

/** The rates for a model id, or null when the catalog doesn't know it or the id
 *  names no specific model. */
export function modelCost(catalog: SlimCatalog, modelId: string | undefined): ModelCost | null {
  const id = modelId?.trim().toLowerCase();
  if (!id || UNPRICEABLE.has(id)) return null;
  return lookupModel(catalog, modelId)?.cost ?? null;
}

/** Dollar cost of `tokens` for `modelId`, or null when the model is unknown to
 *  the catalog or unpriceable. */
export function priceTokens(
  catalog: SlimCatalog,
  modelId: string | undefined,
  tokens: TokenCounts,
): number | null {
  const cost = modelCost(catalog, modelId);
  if (!cost) return null;
  return (
    (tokens.input * cost.input +
      tokens.output * cost.output +
      tokens.cacheRead * cost.cacheRead +
      tokens.cacheWrite * cost.cacheWrite) /
    PER_MILLION
  );
}

/** What the cache reads would have cost at the full input rate, minus what they
 *  actually cost. Zero for a provider that doesn't discount cache reads; null
 *  when the model is unpriceable. */
export function cacheSavingsUsd(
  catalog: SlimCatalog,
  modelId: string | undefined,
  tokens: TokenCounts,
): number | null {
  const cost = modelCost(catalog, modelId);
  if (!cost) return null;
  return (tokens.cacheRead * Math.max(0, cost.input - cost.cacheRead)) / PER_MILLION;
}
