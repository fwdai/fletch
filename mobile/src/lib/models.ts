// Model choices for the pickers. `discover_supported_models` is the truth, but
// it needs the agent CLIs installed and probed on the host, so a small static
// list stands in until it answers.

import type { AgentModels, DiscoveredModel } from "@desktop/data/modelCatalog/types";
import { PROVIDERS } from "@desktop/data/providers";
import { useEffect, useState } from "react";
import { api, useStore } from "../store";

/** Empty id = "let the CLI pick", which is what a null `model` means to the
 *  host. Always offered, since discovery only reports concrete ids. */
export const DEFAULT_MODEL: DiscoveredModel = { id: "", name: "Default model" };

export const STATIC_MODELS: AgentModels[] = [
  {
    agent: "claude",
    models: [
      { id: "claude-opus-4-5", name: "Claude Opus 4.5" },
      { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5" },
      { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
    ],
  },
  { agent: "codex", models: [{ id: "gpt-5-codex", name: "GPT-5 Codex" }] },
  { agent: "cursor", models: [] },
  { agent: "opencode", models: [] },
  { agent: "pi", models: [] },
  { agent: "antigravity", models: [] },
];

// Cached for the run once the host answers. A fallback result is never cached,
// so a discovery that ran before the connection was up gets another chance.
let cache: AgentModels[] | null = null;
let inFlight: Promise<AgentModels[]> | null = null;

function fetchModels(): Promise<AgentModels[]> {
  if (cache) return Promise.resolve(cache);
  inFlight ??= api
    .discoverSupportedModels()
    .then((list) => {
      if (list.length) cache = list;
      return cache ?? STATIC_MODELS;
    })
    .catch(() => STATIC_MODELS)
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/** Discovered models, falling back to the static list. Retried when the
 *  connection comes up, since discovery is a host round-trip. */
export function useModels(): AgentModels[] {
  const connection = useStore((s) => s.connection);
  const [models, setModels] = useState<AgentModels[]>(cache ?? STATIC_MODELS);
  useEffect(() => {
    if (connection !== "connected") return;
    let live = true;
    void fetchModels().then((list) => live && setModels(list));
    return () => {
      live = false;
    };
  }, [connection]);
  return models;
}

export const modelsFor = (models: AgentModels[], provider: string): DiscoveredModel[] => {
  const found =
    models.find((m) => m.agent === provider)?.models ??
    STATIC_MODELS.find((m) => m.agent === provider)?.models ??
    [];
  return [DEFAULT_MODEL, ...found];
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
export function effortsFor(model: DiscoveredModel | undefined): string[] {
  return model?.reasoningLevels?.length ? model.reasoningLevels : ["low", "medium", "high", "max"];
}
