// Assemble the unified catalog from agent discovery + models.dev enrichment.
//
// For each agent:
//   - providerHint set (claude/antigravity) → expand that models.dev provider.
//   - otherwise → use the CLI-reported ids, enriching each with models.dev
//     metadata, falling back to whatever the CLI reported for ids models.dev
//     doesn't know (e.g. OpenCode's free "zen" models).
// Every model lands in `byId` (metadata lookup) and under its agent in
// `byAgent` (the future picker).

import type { AgentModels, DiscoveredModel, ModelMeta, UnifiedCatalog } from "./types";
import type { ModelsDevIndex } from "./modelsDev";
import { modelIdCandidates } from "./normalize";

/** models.dev metadata for an id, trying the normalizer's candidate keys. */
function fromModelsDev(index: ModelsDevIndex, id: string): ModelMeta | undefined {
  for (const key of modelIdCandidates(id)) {
    const hit = index.byId[key];
    if (hit) return hit;
  }
  return undefined;
}

/** Resolve a discovered model's metadata: models.dev wins, the CLI fills gaps. */
function metaFor(d: DiscoveredModel, index: ModelsDevIndex): ModelMeta {
  const dev = fromModelsDev(index, d.id);
  return {
    name: dev?.name ?? d.name ?? d.id,
    contextWindow: dev?.contextWindow || d.contextWindow || 0,
    reasoning: dev?.reasoning ?? d.reasoning ?? false,
  };
}

export function buildCatalog(agents: AgentModels[], index: ModelsDevIndex): UnifiedCatalog {
  const byId: Record<string, ModelMeta> = {};
  const byAgent: Record<string, ModelMeta[]> = {};

  for (const { agent, providerHint, models } of agents) {
    const entries: Array<[string, ModelMeta]> = providerHint
      ? (index.byProvider[providerHint] ?? []).map((id) => [id, index.byId[id]])
      : models.map((d) => [d.id, metaFor(d, index)]);

    const list: ModelMeta[] = [];
    for (const [id, meta] of entries) {
      if (!meta) continue;
      byId[id] = byId[id] ?? meta; // first writer wins; agents share metadata
      list.push(meta);
    }
    byAgent[agent] = list;
  }

  return { byId, byAgent };
}
