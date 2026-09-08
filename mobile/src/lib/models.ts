// Model choices for the pickers.
//
// Same pipeline as the desktop: the host says which models each agent CLI
// supports, and the frontend enriches those ids against models.dev — including
// expanding the `providerHint` agents that have no list command at all (claude
// reports zero models and "anthropic" instead). Building that here rather than
// reading the raw discovery is what makes the Claude model picker non-empty.
//
// models.dev is a phone-side fetch, so a device with no WAN keeps the small
// static list below as a stand-in.

import { loadCachedCatalog, refreshCatalog } from "@desktop/data/modelCatalog";
import type { ModelMeta } from "@desktop/data/modelCatalog/types";
import { PROVIDERS } from "@desktop/data/providers";
import { useEffect, useState } from "react";
import { api, useStore } from "../store";

/** A provider's selectable models, keyed by agent id — the catalog's `byAgent`
 *  view, which is all the pickers need. */
export type ModelsByAgent = Record<string, ModelMeta[]>;

/** Empty id = "let the CLI pick", which is what a null `model` means to the
 *  host. Always offered, since discovery only reports concrete ids. */
export const DEFAULT_MODEL: ModelMeta = {
  id: "",
  name: "Default model",
  contextWindow: 0,
  reasoning: false,
};

/** Offline stand-in, used per provider when the catalog has nothing for it.
 *  Providers absent here (cursor, opencode, pi, antigravity) offer only the
 *  default row until the catalog answers. */
export const STATIC_MODELS: ModelsByAgent = {
  claude: [
    { id: "claude-opus-5", name: "Claude Opus 5", contextWindow: 1_000_000, reasoning: true },
    { id: "claude-sonnet-5", name: "Claude Sonnet 5", contextWindow: 1_000_000, reasoning: true },
    { id: "claude-haiku-4-5", name: "Claude Haiku 4.5", contextWindow: 200_000, reasoning: false },
  ],
  codex: [{ id: "gpt-5-codex", name: "GPT-5 Codex", contextWindow: 0, reasoning: true }],
};

// Last catalog this run, so a picker mounted later starts from the freshest
// value rather than re-reading (and re-parsing) the localStorage cache.
let current: ModelsByAgent = loadCachedCatalog().byAgent;

/** The catalog's per-agent model lists. Refreshed when the connection comes up,
 *  since discovery is a host round-trip; `refreshCatalog` dedupes the concurrent
 *  calls this makes when several pickers are mounted, and honours its own cache
 *  TTL, so this is cheap on every mount but the first. */
export function useModels(): ModelsByAgent {
  const connection = useStore((s) => s.connection);
  const [models, setModels] = useState<ModelsByAgent>(current);
  useEffect(() => {
    if (connection !== "connected") return;
    let live = true;
    void refreshCatalog(api.discoverSupportedModels).then((catalog) => {
      if (!catalog) return;
      current = catalog.byAgent;
      if (live) setModels(catalog.byAgent);
    });
    return () => {
      live = false;
    };
  }, [connection]);
  return models;
}

/** A provider's rows for a picker: the default first, then its models. An empty
 *  catalog entry falls back to the static list — an agent whose CLI has no list
 *  command reports zero models, and offering only "Default model" would leave
 *  the picker with nothing to pick. */
export const modelsFor = (models: ModelsByAgent, provider: string): ModelMeta[] => {
  const found = models[provider]?.length ? models[provider] : STATIC_MODELS[provider];
  return [DEFAULT_MODEL, ...(found ?? [])];
};

/** Providers offered in the new-agent sheet. Providers that manage their own
 *  model still spawn — they just show only the default row. */
export const providerOptions = () => PROVIDERS;

/** Context window as a chip label: "1M ctx" / "200k ctx"; null when unknown. */
export function contextLabel(tokens: number | undefined): string | null {
  if (!tokens) return null;
  return tokens >= 1_000_000
    ? `${Math.round(tokens / 1_000_000)}M ctx`
    : `${Math.round(tokens / 1000)}k ctx`;
}

/** Effort levels the chosen model reports, else the shared ladder. */
export function effortsFor(model: ModelMeta | undefined): string[] {
  return model?.reasoningLevels?.length ? model.reasoningLevels : ["low", "medium", "high", "max"];
}
