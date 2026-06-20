// Assemble the unified catalog from agent discovery + models.dev enrichment.
//
// For each agent:
//   - providerHint set (claude) → expand that models.dev provider.
//   - otherwise → use the CLI-reported ids, enriching each with models.dev
//     metadata, falling back to whatever the CLI reported for ids models.dev
//     doesn't know (e.g. OpenCode's free "zen" models).
// Every model lands in `byId` (metadata lookup) and under its agent in
// `byAgent` (the future picker).

import type { AgentModels, DiscoveredModel, ModelMeta, UnifiedCatalog } from "./types";
import type { ModelsDevIndex } from "./modelsDev";
import { lookupModel } from "./normalize";

function releaseRank(meta: ModelMeta): number {
  if (!meta.releaseDate) return 0;
  const time = Date.parse(meta.releaseDate);
  return Number.isFinite(time) ? time : 0;
}

function sortByReleaseDateDesc(models: ModelMeta[]): ModelMeta[] {
  return models
    .map((meta, index) => ({ meta, index }))
    .sort((a, b) => {
      const byDate = releaseRank(b.meta) - releaseRank(a.meta);
      return byDate || a.index - b.index;
    })
    .map(({ meta }) => meta);
}

// Shared curation primitives. Every agent's model list is curated by the same
// shape — collapse near-duplicate releases to one representative, then keep only
// the newest few per class — so both steps live here and the per-agent curators
// supply the classifier/scorer/caps that genuinely differ between providers.

/** Collapse models sharing a key down to a single representative, keeping the
 *  one `prefer` ranks highest. A null key drops the model. The survivor keeps
 *  its group's first-seen position, so a release-desc input stays release-desc. */
export function dedupeBest(
  models: ModelMeta[],
  keyFn: (meta: ModelMeta) => string | null,
  prefer: (candidate: ModelMeta, current: ModelMeta) => boolean,
): ModelMeta[] {
  const byKey = new Map<string, ModelMeta>();
  const order: string[] = [];
  for (const meta of models) {
    const key = keyFn(meta);
    if (key === null) continue;
    const current = byKey.get(key);
    if (!current) {
      byKey.set(key, meta);
      order.push(key);
    } else if (prefer(meta, current)) {
      byKey.set(key, meta);
    }
  }
  return order.map((key) => byKey.get(key) as ModelMeta);
}

/** Keep at most `caps[group]` models per group, in the order given. An
 *  unclassifiable model (null group) is dropped. Feed a release-desc list to
 *  keep the newest N per group. */
export function capPerGroup<G extends string>(
  models: ModelMeta[],
  groupFn: (meta: ModelMeta) => G | null,
  caps: Record<G, number>,
): ModelMeta[] {
  const counts = {} as Record<G, number>;
  return models.filter((meta) => {
    const group = groupFn(meta);
    if (group === null) return false;
    const used = counts[group] ?? 0;
    if (used >= caps[group]) return false;
    counts[group] = used + 1;
    return true;
  });
}

function cleanModelName(name: string): string {
  return name.replace(/(?:\s*\((?:default|latest)\))*\s*$/gi, "").trim();
}

function displayModel(meta: ModelMeta): ModelMeta {
  const name = cleanModelName(meta.name);
  return name === meta.name ? meta : { ...meta, name };
}

function strippedCursorModelIds(id: string): string[] {
  const out: string[] = [];
  const add = (v: string) => {
    if (v && !out.includes(v)) out.push(v);
  };
  add(id);

  let stripped = id;
  let changed = true;
  while (changed) {
    const next = stripped
      .replace(/-fast$/, "")
      .replace(/-thinking$/, "")
      .replace(/-thinking-(low|medium|high|xhigh|max)$/, "")
      .replace(/-(none|low|medium|high|xhigh|max|extra-high)$/, "");
    changed = next !== stripped;
    stripped = next;
    add(stripped);
  }

  const claude = stripped.match(
    /^claude-(?:(opus|sonnet|haiku)-(\d)-(\d)|(\d)\.(\d)-(opus|sonnet|haiku))/,
  );
  if (claude) {
    const family = claude[1] ?? claude[6];
    const major = claude[2] ?? claude[4];
    const minor = claude[3] ?? claude[5];
    add(`claude-${family}-${major}-${minor}`);
  }

  return out;
}

function lookupEnrichedModel(index: ModelsDevIndex, id: string): ModelMeta | undefined {
  for (const candidate of strippedCursorModelIds(id)) {
    const meta = lookupModel(index.byId, candidate);
    if (meta) return meta;
  }
  return undefined;
}

function claudeFamily(meta: ModelMeta): "opus" | "sonnet" | "haiku" | null {
  const haystack = `${meta.id} ${meta.name}`.toLowerCase();
  if (haystack.includes("opus")) return "opus";
  if (haystack.includes("sonnet")) return "sonnet";
  if (haystack.includes("haiku")) return "haiku";
  return null;
}

function claudeFamilyRank(meta: ModelMeta): number {
  const family = claudeFamily(meta);
  if (family === "opus") return 0;
  if (family === "sonnet") return 1;
  if (family === "haiku") return 2;
  return 3;
}

function claudeMajor(meta: ModelMeta): number {
  const m = `${meta.id} ${meta.name}`.match(/(?:claude[ -])?(?:opus|sonnet|haiku)[ -](\d+)/i);
  return m ? Number(m[1]) : 0;
}

function claudeDedupeKey(meta: ModelMeta): string {
  return meta.name
    .replace(/\s*\(latest\)\s*/gi, "")
    .trim()
    .toLowerCase();
}

function latestAliasScore(meta: ModelMeta): number {
  if (/\(latest\)/i.test(meta.name)) return 2;
  if (!/-\d{8}$/.test(meta.id)) return 1;
  return 0;
}

/** Prefer the `(latest)` alias over a dated id, then the newer release. */
function preferClaudeRelease(candidate: ModelMeta, current: ModelMeta): boolean {
  const byAlias = latestAliasScore(candidate) - latestAliasScore(current);
  return byAlias > 0 || (byAlias === 0 && releaseRank(candidate) > releaseRank(current));
}

function curateClaudeModels(models: ModelMeta[]): ModelMeta[] {
  const caps = { opus: 3, sonnet: 2, haiku: 1 };

  const deduped = dedupeBest(models, claudeDedupeKey, preferClaudeRelease).filter(
    (meta) => claudeFamily(meta) !== null && claudeMajor(meta) >= 4,
  );

  return capPerGroup(deduped, claudeFamily, caps).sort((a, b) => {
    const byFamily = claudeFamilyRank(a) - claudeFamilyRank(b);
    return byFamily || releaseRank(b) - releaseRank(a);
  });
}

// Pi is a multi-provider router whose list is dominated by the full Claude
// lineage (v3 → v4, dated + (latest) aliases). Reuse the Claude family
// grouping for its Anthropic models and keep any other providers after them,
// already release-desc ordered by buildCatalog.
function curatePiModels(models: ModelMeta[]): ModelMeta[] {
  const claude = models.filter((meta) => claudeFamily(meta) !== null);
  const others = models.filter((meta) => claudeFamily(meta) === null);
  return [...curateClaudeModels(claude), ...others];
}

type CursorProvider = "anthropic" | "openai" | "google" | "xai" | "kimi";

function cursorComposerVersion(meta: ModelMeta): number {
  const m = meta.id.match(/^composer-(\d+(?:\.\d+)?)/);
  return m ? Number(m[1]) : 0;
}

function cursorProvider(meta: ModelMeta): CursorProvider | null {
  const text = `${meta.id} ${meta.name}`.toLowerCase();
  if (text.includes("claude") || /\b(opus|sonnet|haiku|fable)\b/.test(text)) return "anthropic";
  if (text.includes("gpt") || text.includes("codex") || /^o\d/.test(meta.id)) return "openai";
  if (text.includes("gemini")) return "google";
  if (text.includes("grok")) return "xai";
  if (text.includes("kimi")) return "kimi";
  return null;
}

function cursorProviderRank(provider: CursorProvider): number {
  if (provider === "anthropic") return 0;
  if (provider === "openai") return 1;
  if (provider === "google") return 2;
  if (provider === "xai") return 3;
  return 4;
}

function cursorBaseKey(meta: ModelMeta): string {
  return meta.name
    .replace(/\s*\([^)]*\)\s*/g, " ")
    .replace(/\b1m\b/gi, " ")
    .replace(/\bno zdr\b/gi, " ")
    .replace(/\b(low|medium|high|extra high|max|none|fast|thinking|default)\b/gi, " ")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();
}

function cursorVariantScore(meta: ModelMeta): number {
  const text = `${meta.id} ${meta.name}`.toLowerCase();
  const hasEffortSuffix = /-(none|low|medium|high|xhigh|max|extra-high)(?:-|$)/.test(meta.id);
  const nameHasVariant = /\b(low|medium|high|extra high|max|none|fast|thinking)\b/i.test(meta.name);
  let score = 0;
  if (/\(default\)/i.test(meta.name)) score += 30;
  if (!nameHasVariant) score += 20;
  if (!hasEffortSuffix) score += 10;
  if (/-medium(?:-|$)/.test(meta.id)) score += 6;
  if (!text.includes("thinking")) score += 8;
  if (!text.includes("fast")) score += 4;
  return score;
}

const CURSOR_CAPS: Record<CursorProvider, number> = {
  anthropic: 3,
  openai: 3,
  google: 3,
  xai: 2,
  kimi: 2,
};

/** Group key that keeps providers separate so the same base name under two
 *  providers can't collide. Null when the model isn't a known provider/base. */
function cursorProviderBaseKey(meta: ModelMeta): string | null {
  const provider = cursorProvider(meta);
  const base = cursorBaseKey(meta);
  return provider && base ? `${provider}::${base}` : null;
}

function cursorBaseRepresentatives(models: ModelMeta[]): ModelMeta[] {
  const reps = dedupeBest(
    models,
    cursorProviderBaseKey,
    (candidate, current) => cursorVariantScore(candidate) > cursorVariantScore(current),
  );

  // `reps` is in first-seen (release-desc) order, so capPerGroup keeps the
  // newest per provider and the stable sort below preserves that order on ties.
  return capPerGroup(reps, cursorProvider, CURSOR_CAPS).sort((a, b) => {
    const byProvider = cursorProviderRank(cursorProvider(a) as CursorProvider) -
      cursorProviderRank(cursorProvider(b) as CursorProvider);
    if (byProvider) return byProvider;
    if (cursorProvider(a) === "anthropic") {
      const byFamily = claudeFamilyRank(a) - claudeFamilyRank(b);
      if (byFamily) return byFamily;
    }
    return releaseRank(b) - releaseRank(a);
  });
}

function curateCursorModels(models: ModelMeta[]): ModelMeta[] {
  const composerVersions = [
    ...new Set(
      models
        .filter((meta) => meta.id.startsWith("composer-"))
        .map((meta) => cursorComposerVersion(meta))
        .filter((version) => version > 0),
    ),
  ]
    .sort((a, b) => b - a)
    .slice(0, 3);
  const composer = models
    .filter((meta) => composerVersions.includes(cursorComposerVersion(meta)))
    .sort((a, b) => {
      const byVersion = cursorComposerVersion(b) - cursorComposerVersion(a);
      if (byVersion) return byVersion;
      const byDefault = Number(/\(default\)/i.test(b.name)) - Number(/\(default\)/i.test(a.name));
      if (byDefault) return byDefault;
      return Number(b.id.includes("fast")) - Number(a.id.includes("fast"));
    });

  return [...composer, ...cursorBaseRepresentatives(models.filter((meta) => !meta.id.startsWith("composer-")))];
}

/** Resolve a discovered model's metadata: models.dev wins, the CLI fills gaps.
 *  `index.byId` is a SlimCatalog, so the shared normalizer does the matching. */
function metaFor(d: DiscoveredModel, index: ModelsDevIndex): ModelMeta {
  const dev = lookupEnrichedModel(index, d.id);
  return {
    id: d.id,
    name: dev?.name ?? d.name ?? d.id,
    contextWindow: dev?.contextWindow || d.contextWindow || 0,
    reasoning: dev?.reasoning ?? d.reasoning ?? false,
    ...(dev?.releaseDate ? { releaseDate: dev.releaseDate } : {}),
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
      const entry = meta.id === id ? meta : { ...meta, id };
      const displayEntry = displayModel(entry);
      byId[id] = byId[id] ?? displayEntry; // first writer wins; agents share metadata
      if (id.includes("/")) {
        const bare = id.split("/").pop();
        if (bare) byId[bare] = byId[bare] ?? { ...displayEntry, id: bare };
      }
      list.push(entry);
    }
    const sorted = sortByReleaseDateDesc(list);
    const agentModels =
      agent === "claude"
        ? curateClaudeModels(sorted)
        : agent === "pi"
          ? curatePiModels(sorted)
        : agent === "cursor"
          ? curateCursorModels(sorted)
          : sorted;
    byAgent[agent] = agentModels.map(displayModel);
  }

  return { byId, byAgent };
}
