// Resolve a model id reported by an agent CLI to a catalog entry.
//
// CLI-reported ids don't always equal models.dev keys: some carry a provider
// prefix ("anthropic/claude-opus-4-8"), some a date suffix that the catalog may
// or may not key on. We try a sequence of candidate keys, most-specific first,
// and return the first hit. An unmatched id returns undefined so callers fall
// back to their existing defaults — nothing regresses.

import type { ModelMeta, SlimCatalog } from "./types";

/** Ordered candidate keys to try for a raw model id. */
export function modelIdCandidates(rawId: string): string[] {
  const candidates: string[] = [];
  const add = (id: string) => {
    const v = id.trim();
    if (v && !candidates.includes(v)) candidates.push(v);
  };

  const id = rawId.trim();
  add(id);

  // Strip a provider prefix: "anthropic/claude-opus-4-8" -> "claude-opus-4-8".
  const bare = id.includes("/") ? id.slice(id.lastIndexOf("/") + 1) : id;
  add(bare);

  // Strip a trailing bracketed variant tag — Claude Code reports its 1M-context
  // models as "claude-opus-4-8[1m]", which no catalog key carries.
  const untagged = bare.replace(/\[[^\]]*\]$/, "");
  add(untagged);

  // Strip a trailing release date ("-20250514"): the catalog often keys on the
  // undated base id as well. Apply after the tag strip so both can compose
  // (e.g. "claude-haiku-4-5-20251001[1m]" -> "claude-haiku-4-5").
  const undated = untagged.replace(/-\d{8}$/, "");
  add(undated);

  // Lowercased variants, for case-insensitive matching.
  add(untagged.toLowerCase());
  add(undated.toLowerCase());

  return candidates;
}

/** Look up metadata for an agent-reported model id, or undefined if unknown. */
export function lookupModel(
  catalog: SlimCatalog,
  rawId: string | undefined,
): ModelMeta | undefined {
  if (!rawId) return undefined;
  for (const key of modelIdCandidates(rawId)) {
    const hit = catalog[key];
    if (hit) return hit;
  }
  return undefined;
}

/** Look up a model within one provider's list, using the same id-candidate
 *  matching as `lookupModel`. Prefer this over the global `byId` view when the
 *  caller knows the provider: a model id shared across agents keeps only the
 *  first-discovered entry in `byId`, which may belong to another provider and
 *  lack this provider's metadata (e.g. codex's per-model reasoning levels). */
export function lookupModelInList(
  models: ModelMeta[] | undefined,
  rawId: string | undefined,
): ModelMeta | undefined {
  if (!models || !rawId) return undefined;
  for (const key of modelIdCandidates(rawId)) {
    const hit = models.find((m) => m.id === key);
    if (hit) return hit;
  }
  return undefined;
}
