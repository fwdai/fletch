// models.dev access — the metadata source (context window, reasoning) the
// agent CLIs don't all report. We fetch the full api.json once per rebuild and
// index it two ways: by bare model id (to enrich a discovered id) and by
// provider (to expand the provider hints for agents with no list command).
//
// CORS is open on models.dev and the Tauri webview CSP allow-lists
// https://models.dev in connect-src, so the frontend fetches it directly — no
// backend round-trip.

import type { ModelMeta } from "./types";

const MODELS_DEV_URL = "https://models.dev/api.json";

interface RawModel {
  name?: string;
  reasoning?: boolean;
  family?: string;
  release_date?: string;
  limit?: { context?: number };
}

/** A lookup over models.dev: metadata by bare model id, and the model ids each
 *  provider offers. */
export interface ModelsDevIndex {
  byId: Record<string, ModelMeta>;
  byProvider: Record<string, string[]>;
}

function toMeta(id: string, m: RawModel): ModelMeta {
  return {
    id,
    name: m.name ?? id,
    contextWindow: m.limit?.context ?? 0,
    reasoning: m.reasoning === true,
    ...(m.family ? { family: m.family } : {}),
    ...(m.release_date ? { releaseDate: m.release_date } : {}),
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
