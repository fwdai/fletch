// Types for the hybrid model catalog.
//
// Each agent CLI is asked which models it supports (DiscoveredModel/AgentModels,
// mirrored from the Rust `discover_supported_models` command). Those ids are
// enriched against models.dev into ModelMeta and assembled into a UnifiedCatalog
// — a flat id→meta map (`byId`, for the usage gauge) plus per-agent lists
// (`byAgent`, for the future model picker).

/** Metadata for one model, keyed in `byId` by the id used for lookup/launch. */
export interface ModelMeta {
  /** Bare model id as passed to the agent CLI (e.g. "claude-opus-4-8"). */
  id: string;
  /** Human-facing model name (e.g. "Claude Opus 4.5"). */
  name: string;
  /** Context window in tokens. 0 when unknown. */
  contextWindow: number;
  /** Whether the model supports reasoning / extended thinking. */
  reasoning: boolean;
  /** Model release date from models.dev (`YYYY-MM-DD`), when known. */
  releaseDate?: string;
}

/** One model an agent reports it supports (from the Rust discovery command).
 *  Optional fields are present only when the CLI itself reports them. */
export interface DiscoveredModel {
  id: string;
  name?: string;
  contextWindow?: number;
  reasoning?: boolean;
}

/** An agent and the models it supports. `providerHint` is set for agents with
 *  no list command (claude→"anthropic", antigravity→"google") — the frontend
 *  expands that models.dev provider instead. */
export interface AgentModels {
  agent: string;
  providerHint?: string;
  models: DiscoveredModel[];
}

/** Flat id→meta map (e.g. "claude-opus-4-8" → {…}). Drives metadata lookup. */
export type SlimCatalog = Record<string, ModelMeta>;

/** The assembled catalog: metadata by id, plus the per-agent model lists. */
export interface UnifiedCatalog {
  byId: SlimCatalog;
  byAgent: Record<string, ModelMeta[]>;
}
