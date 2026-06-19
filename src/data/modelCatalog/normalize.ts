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

  // Strip a trailing release date ("-20250514"): the catalog often keys on the
  // undated base id as well.
  const undated = bare.replace(/-\d{8}$/, "");
  add(undated);

  // Lowercased variants, for case-insensitive matching.
  add(bare.toLowerCase());
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
