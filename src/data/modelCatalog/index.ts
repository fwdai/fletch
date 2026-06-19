// Public entrypoint for the model-metadata catalog.
//
// Resilience model (packaged resource + background refresh):
//   1. A successful live fetch is cached in localStorage; on later launches the
//      cache is the synchronous baseline, even offline.
//   2. With no cache yet (first run), the store loads the snapshot packaged with
//      the app — read from disk via a Tauri command, NOT bundled into the JS —
//      so metadata works offline on the very first launch.
//   3. On startup the store refreshes from models.dev in the background and
//      updates both the cache and live state.
//
// New models therefore appear without an app release: once models.dev lists a
// model, the next launch's refresh (or the current session's, on success)
// picks it up.

import { api } from "../../api";
import type { SlimCatalog } from "./types";
import { slimFullCatalog } from "./slim";

export type { ModelMeta, SlimCatalog } from "./types";
export { lookupModel } from "./normalize";

const MODELS_DEV_URL = "https://models.dev/api.json";
const CACHE_KEY = "modelCatalog.cache.v1";

interface CacheEnvelope {
  fetchedAt: number;
  catalog: SlimCatalog;
}

/** Catalog available synchronously without disk or network: a previously-fetched
 *  copy from localStorage, or an empty map. The store seeds from the packaged
 *  resource (loadPackagedCatalog) when this is empty. Synchronous so the store
 *  has real data on first render for returning users. */
export function loadCachedCatalog(): SlimCatalog {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (raw) {
      const env = JSON.parse(raw) as CacheEnvelope;
      if (env?.catalog && Object.keys(env.catalog).length > 0) return env.catalog;
    }
  } catch {
    // Corrupt cache — fall through to an empty catalog.
  }
  return {};
}

/** Read the snapshot packaged with the app (a Tauri resource on disk). Returns
 *  null on any failure so the caller keeps whatever it already has. */
export async function loadPackagedCatalog(): Promise<SlimCatalog | null> {
  try {
    const text = await api.readBundledModelCatalog();
    const catalog = JSON.parse(text) as SlimCatalog;
    return Object.keys(catalog).length > 0 ? catalog : null;
  } catch {
    return null;
  }
}

/** Fetch the latest catalog from models.dev, slim it, and persist to the cache.
 *  Returns the fresh catalog, or null on any failure (offline, parse error) so
 *  the caller keeps whatever it already has. */
export async function refreshCatalog(): Promise<SlimCatalog | null> {
  try {
    const res = await fetch(MODELS_DEV_URL);
    if (!res.ok) return null;
    const apiJson = (await res.json()) as Record<string, unknown>;
    const catalog = slimFullCatalog(apiJson);
    if (Object.keys(catalog).length === 0) return null;
    try {
      const env: CacheEnvelope = { fetchedAt: Date.now(), catalog };
      localStorage.setItem(CACHE_KEY, JSON.stringify(env));
    } catch {
      // Storage full / unavailable — the in-memory result is still usable.
    }
    return catalog;
  } catch {
    return null;
  }
}
