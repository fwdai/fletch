// Reduce the full models.dev api.json down to the slim `SlimCatalog` the UI
// uses. Shared by the build-time snapshot generator and the runtime refresh so
// the bundled baseline and live data always have the same shape.

import type { ModelMeta, SlimCatalog } from "./types";

/** Providers whose model definitions are canonical. Listed first so that on a
 *  bare-id collision (routers like opencode/vercel re-list the same model ids)
 *  the first-party definition wins and routers only add ids no canonical
 *  provider defines. */
const CANONICAL_PROVIDERS = [
  "anthropic",
  "openai",
  "google",
  "google-vertex",
  "google-vertex-anthropic",
];

interface RawModel {
  name?: string;
  reasoning?: boolean;
  limit?: { context?: number };
}

interface RawProvider {
  models?: Record<string, RawModel>;
}

/** Transform the full api.json object into a slim, flat-by-model-id catalog.
 *  Accepts `unknown`-valued input because the source is untrusted external
 *  JSON; every field is read defensively below, so the one narrowing cast per
 *  provider is safe. */
export function slimFullCatalog(api: Record<string, unknown>): SlimCatalog {
  const out: SlimCatalog = {};
  const providerIds = Object.keys(api);
  // Canonical providers first, then the rest — first writer of an id wins.
  const ordered = [
    ...CANONICAL_PROVIDERS.filter((id) => id in api),
    ...providerIds.filter((id) => !CANONICAL_PROVIDERS.includes(id)),
  ];

  for (const providerId of ordered) {
    // Narrowed from `unknown`; fields below are all optional-accessed.
    const models = (api[providerId] as RawProvider | undefined)?.models;
    if (!models) continue;
    for (const [modelId, m] of Object.entries(models)) {
      if (modelId in out) continue; // canonical / first definition wins
      const meta: ModelMeta = {
        name: m.name ?? modelId,
        contextWindow: m.limit?.context ?? 0,
        reasoning: m.reasoning === true,
      };
      out[modelId] = meta;
    }
  }
  return out;
}
