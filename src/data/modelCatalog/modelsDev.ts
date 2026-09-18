// models.dev access — the metadata source (context window, reasoning) the
// agent CLIs don't all report. We fetch the full api.json once per rebuild and
// index it two ways: by bare model id (to enrich a discovered id) and by
// provider (to expand the provider hints for agents with no list command).
//
// CORS is open on models.dev and the Tauri webview CSP allow-lists
// https://models.dev in connect-src, so the frontend fetches it directly — no
// backend round-trip.

import type { ModelCost } from "./pricing";
import type { ModelMeta } from "./types";

const MODELS_DEV_URL = "https://models.dev/api.json";

/** models.dev prices a model in USD per MILLION tokens. Cache rates are
 *  optional — a provider that charges cache traffic as ordinary input lists
 *  neither. The per-tier overrides (`tiers`, `context_over_200k`) are ignored;
 *  everything here is the base tier. */
interface RawCost {
  input?: number;
  output?: number;
  cache_read?: number;
  cache_write?: number;
}

interface RawModel {
  name?: string;
  reasoning?: boolean;
  family?: string;
  release_date?: string;
  limit?: { context?: number };
  cost?: RawCost;
}

/** Map models.dev's `cost` block to the catalog's rates, defaulting the cache
 *  rates to the input rate. Undefined when models.dev prices neither side of
 *  the call — an unpriced entry must read as "unknown", never as free. */
function toCost(cost: RawCost | undefined): ModelCost | undefined {
  const input = typeof cost?.input === "number" ? cost.input : undefined;
  const output = typeof cost?.output === "number" ? cost.output : undefined;
  if (input === undefined && output === undefined) return undefined;
  const base = input ?? 0;
  return {
    input: base,
    output: output ?? 0,
    cacheRead: typeof cost?.cache_read === "number" ? cost.cache_read : base,
    cacheWrite: typeof cost?.cache_write === "number" ? cost.cache_write : base,
  };
}

/** A lookup over models.dev: metadata by bare model id, and the model ids each
 *  provider offers. */
export interface ModelsDevIndex {
  byId: Record<string, ModelMeta>;
  byProvider: Record<string, string[]>;
}

function toMeta(id: string, m: RawModel): ModelMeta {
  const cost = toCost(m.cost);
  return {
    id,
    name: m.name ?? id,
    contextWindow: m.limit?.context ?? 0,
    reasoning: m.reasoning === true,
    ...(m.family ? { family: m.family } : {}),
    ...(m.release_date ? { releaseDate: m.release_date } : {}),
    ...(cost ? { cost } : {}),
  };
}

/** Index a parsed api.json. Canonical providers (anthropic/openai/google) are
 *  read first so they win over routers on a bare-id collision. */
export function indexModelsDev(api: Record<string, unknown>): ModelsDevIndex {
  const byId: Record<string, ModelMeta> = {};
  const byProvider: Record<string, string[]> = {};
  const canonical = ["anthropic", "openai", "google", "google-vertex"];
  const ordered = [
    ...canonical.filter((p) => p in api),
    ...Object.keys(api).filter((p) => !canonical.includes(p)),
  ];

  for (const provider of ordered) {
    const models = (api[provider] as { models?: Record<string, RawModel> })?.models;
    if (!models) continue;
    const ids: string[] = [];
    for (const [id, m] of Object.entries(models)) {
      ids.push(id);
      if (!(id in byId)) byId[id] = toMeta(id, m);
    }
    byProvider[provider] = ids;
  }
  return { byId, byProvider };
}

/** Fetch + index models.dev. Returns null on any failure (offline, parse). */
export async function fetchModelsDevIndex(): Promise<ModelsDevIndex | null> {
  try {
    const res = await fetch(MODELS_DEV_URL);
    if (!res.ok) return null;
    const api = (await res.json()) as Record<string, unknown>;
    const index = indexModelsDev(api);
    return Object.keys(index.byId).length > 0 ? index : null;
  } catch {
    return null;
  }
}
