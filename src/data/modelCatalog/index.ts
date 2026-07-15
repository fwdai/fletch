// Public entrypoint for the hybrid model catalog.
//
// Pipeline: ask each agent CLI which models it supports (Rust discovery) →
// enrich those ids against models.dev → assemble a UnifiedCatalog (id→meta for
// the usage gauge, per-agent lists for the future picker). The result is cached
// in localStorage with a 1h TTL and rebuilt in the background on expiry, so a
// newly-released model shows up automatically without an app release.

import { api } from "@/api";
import { buildCatalog } from "./build";
import { fetchModelsDevIndex } from "./modelsDev";
import type { UnifiedCatalog } from "./types";

export { lookupModel } from "./normalize";
export type { ModelMeta, SlimCatalog } from "./types";

const CACHE_KEY = "modelCatalog.cache.v13";
const TTL_MS = 60 * 60 * 1000; // 1h

const EMPTY: UnifiedCatalog = { byId: {}, byAgent: {} };

interface CacheEnvelope {
  builtAt: number;
  catalog: UnifiedCatalog;
}

let refreshInFlight: Promise<UnifiedCatalog | null> | null = null;

function readCache(): CacheEnvelope | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
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
export function loadCachedCatalog(): UnifiedCatalog {
  return readCache()?.catalog ?? EMPTY;
}

/** True when there is no cache or it has aged past the TTL. */
export function isCatalogStale(): boolean {
  const env = readCache();
  return !env || Date.now() - env.builtAt > TTL_MS;
}

/** Rebuild the catalog from agent discovery + models.dev, and cache it. Returns
 *  null on any failure so the caller keeps the last good cache intact. */
export async function rebuildCatalog(): Promise<UnifiedCatalog | null> {
  const [agents, index] = await Promise.all([
    api.discoverSupportedModels().catch(() => []),
    fetchModelsDevIndex(),
  ]);
  if (index === null) return null;
  const catalog = buildCatalog(agents, index);
  if (Object.keys(catalog.byId).length === 0) return null;
  try {
    const env: CacheEnvelope = { builtAt: Date.now(), catalog };
    localStorage.setItem(CACHE_KEY, JSON.stringify(env));
  } catch {
    // Storage unavailable — the in-memory result is still usable this session.
  }
  return catalog;
}

/** Refresh the catalog, deduping concurrent requests so only one rebuild runs.
 *  `force=true` skips the TTL check and is used by the manual developer action. */
export function refreshCatalog(force = false): Promise<UnifiedCatalog | null> {
  if (refreshInFlight) return refreshInFlight;
  if (!force && !isCatalogStale()) return Promise.resolve(loadCachedCatalog());

  refreshInFlight = rebuildCatalog().finally(() => {
    refreshInFlight = null;
  });
  return refreshInFlight;
}
