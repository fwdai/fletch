// Public entrypoint for the hybrid model catalog.
//
// Pipeline: ask each agent CLI which models it supports (Rust discovery) →
// enrich those ids against models.dev → assemble a UnifiedCatalog (id→meta for
// the usage gauge, per-agent lists for the future picker). The result is cached
// in localStorage with a 1h TTL and rebuilt in the background on expiry, so a
// newly-released model shows up automatically without an app release.
//
// One catalog per storage key: the desktop keeps one, under the default; the
// mobile app one per paired host, since half of a catalog is that host's CLIs.

import { buildCatalog } from "./build";
import { fetchModelsDevIndex, type ModelsDevIndex } from "./modelsDev";
import type { AgentModels, UnifiedCatalog } from "./types";

export { modelSummary } from "./build";
export { lookupModel, lookupModelInList } from "./normalize";
export type { ModelCost } from "./pricing";
export { cacheSavingsUsd, modelCost, priceTokens } from "./pricing";
export type { ModelMeta, SlimCatalog } from "./types";

/** How the caller asks the host which models each agent CLI supports. Injected
 *  rather than imported so the mobile app can reuse this whole pipeline: its
 *  discovery is a remote call over the host link, not a Tauri invoke. */
export type DiscoverAgents = () => Promise<AgentModels[]>;

/** The storage key the desktop's catalog is cached under, and the stem of any
 *  other's. */
export const CATALOG_CACHE_KEY = "modelCatalog.cache.v15";
const TTL_MS = 60 * 60 * 1000; // 1h

const EMPTY: UnifiedCatalog = { byId: {}, byAgent: {} };

interface CacheEnvelope {
  builtAt: number;
  catalog: UnifiedCatalog;
}

/** The rebuild out right now, per storage key. */
const refreshesInFlight = new Map<string, Promise<UnifiedCatalog | null>>();

/** The last models.dev read, shared by every rebuild that starts within an
 *  expiry of it: the index is the same whichever catalog is being built. A
 *  forced rebuild reads afresh, and a failed read is not kept. */
let modelsDev: { readAt: number; index: Promise<ModelsDevIndex | null> } | null = null;

function readModelsDev(fresh: boolean): Promise<ModelsDevIndex | null> {
  if (!fresh && modelsDev && Date.now() - modelsDev.readAt <= TTL_MS) return modelsDev.index;
  const read = { readAt: Date.now(), index: fetchModelsDevIndex() };
  modelsDev = read;
  void read.index.then((index) => {
    if (index === null && modelsDev === read) modelsDev = null;
  });
  return read.index;
}

function readCache(key: string): CacheEnvelope | null {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return null;
    const env = JSON.parse(raw) as CacheEnvelope;
    if (env?.catalog?.byId && Object.keys(env.catalog.byId).length > 0) return env;
  } catch {
    // Corrupt cache — treat as absent.
  }
  return null;
}

/** Best catalog available without rebuilding: the cached copy, or empty. Used to
 *  seed the store synchronously so lookups work on the first render. */
export function loadCachedCatalog(key = CATALOG_CACHE_KEY): UnifiedCatalog {
  return readCache(key)?.catalog ?? EMPTY;
}

/** True when there is no cache or it has aged past the TTL. */
export function isCatalogStale(key = CATALOG_CACHE_KEY): boolean {
  const env = readCache(key);
  return !env || Date.now() - env.builtAt > TTL_MS;
}

/** Rebuild the catalog from agent discovery + models.dev, and cache it under
 *  `key`. `fresh` reads models.dev again rather than sharing the last read.
 *  Returns null on any failure so the caller keeps the last good cache intact. */
export async function rebuildCatalog(
  discover: DiscoverAgents,
  key = CATALOG_CACHE_KEY,
  fresh = false,
): Promise<UnifiedCatalog | null> {
  const [agents, index] = await Promise.all([
    discover().catch(() => [] as AgentModels[]),
    readModelsDev(fresh),
  ]);
  if (index === null) return null;
  const catalog = buildCatalog(agents, index);
  if (Object.keys(catalog.byId).length === 0) return null;
  try {
    const env: CacheEnvelope = { builtAt: Date.now(), catalog };
    localStorage.setItem(key, JSON.stringify(env));
  } catch {
    // Storage unavailable — the in-memory result is still usable this session.
  }
  return catalog;
}

/** Refresh the catalog cached under `key`, deduping concurrent requests for it
 *  so only one rebuild runs. `force=true` skips the TTL check and is used by
 *  the manual developer action. */
export function refreshCatalog(
  discover: DiscoverAgents,
  force = false,
  key = CATALOG_CACHE_KEY,
): Promise<UnifiedCatalog | null> {
  const inFlight = refreshesInFlight.get(key);
  if (inFlight) return inFlight;
  if (!force && !isCatalogStale(key)) return Promise.resolve(loadCachedCatalog(key));

  const run = rebuildCatalog(discover, key, force).finally(() => {
    refreshesInFlight.delete(key);
  });
  refreshesInFlight.set(key, run);
  return run;
}
