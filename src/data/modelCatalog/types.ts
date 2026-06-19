// Slim model-metadata catalog derived from models.dev (https://models.dev/api.json).
//
// We keep only the fields the UI consumes — the upstream api.json is ~2.3MB
// across 145 providers, far more than we bundle or ship over the wire. The slim
// shape below is the single contract shared by the build-time snapshot
// generator (scripts/fetch-models-catalog.ts) and the runtime refresh, so both
// produce identical data.

/** Metadata for one model, keyed in `SlimCatalog` by its bare model id. */
export interface ModelMeta {
  /** Human-facing model name from the catalog (e.g. "Claude Opus 4.5"). */
  name: string;
  /** Context window in tokens (`limit.context`). 0 when the catalog omits it. */
  contextWindow: number;
  /** Whether the model supports reasoning / extended thinking. Gates the
   *  composer's thinking-effort picker for known models. */
  reasoning: boolean;
}

/** Flat map keyed by bare model id (e.g. "claude-opus-4-8", "gpt-5.2-codex").
 *  Canonical providers (anthropic/openai/google) win over routers on id
 *  collisions — see slim.ts. */
export type SlimCatalog = Record<string, ModelMeta>;
