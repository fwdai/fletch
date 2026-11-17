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
//
// Every paired host has a catalog of its own: half of one is that host's
// installed CLIs, and with every host connected at once a picker on one must
// neither show another's lists nor throw away another's catalog to build its
// own. Each is the desktop's catalog cache (@desktop/data/modelCatalog) — its
// expiry, its storage and its in-flight rebuild — under a key of the host's.

import { CATALOG_CACHE_KEY, loadCachedCatalog, refreshCatalog } from "@desktop/data/modelCatalog";
import type { ModelMeta } from "@desktop/data/modelCatalog/types";
import { PROVIDERS } from "@desktop/data/providers";
import { useEffect, useMemo, useState } from "react";
import type { KeyedLink } from "../host";
import { useHost } from "../host/useHost";
import { useHostStore } from "../store";

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

/** Where `hostKey`'s catalog is cached. */
const cacheKeyOf = (hostKey: string) => `${CATALOG_CACHE_KEY}:${hostKey}`;

// Where earlier builds kept the phone's catalogs: all of them under one key,
// or (the bare desktop key) whichever host built last. Nothing reads them now.
try {
  for (const key of ["modelCatalog.host", "modelCatalog.byHost", CATALOG_CACHE_KEY]) {
    localStorage.removeItem(key);
  }
} catch {
  // Storage unavailable — there is nothing to clear either.
}

/** `hostKey`'s catalog as last built, or nothing yet. */
export const modelsOf = (hostKey: string): ModelsByAgent =>
  loadCachedCatalog(cacheKeyOf(hostKey)).byAgent;

/** The catalog for `link`'s host: its own, rebuilt when it has none or it has
 *  aged out — one rebuild at a time per host, shared by every picker on it.
 *  Null when the rebuild failed and the caller should keep what it has. */
export async function loadModels({ hostKey, api }: KeyedLink): Promise<ModelsByAgent | null> {
  const catalog = await refreshCatalog(api.discoverSupportedModels, false, cacheKeyOf(hostKey));
  return catalog?.byAgent ?? null;
}

/** Forget `hostKey`'s catalog: the host has been unpaired. */
export function forgetModels(hostKey: string): void {
  try {
    localStorage.removeItem(cacheKeyOf(hostKey));
  } catch {
    // Storage unavailable — nothing was kept.
  }
}

/** The per-agent model lists of the host this component acts on, refreshed
 *  when its connection comes up (discovery is a host round-trip). */
export function useModels(): ModelsByAgent {
  const connection = useHostStore((s) => s.connection);
  const link = useHost();
  // Bumped when a load lands. The lists themselves are read by host, so a
  // picker never shows another host's.
  const [landed, bump] = useState(0);
  useEffect(() => {
    if (connection !== "connected") return;
    let live = true;
    void loadModels(link).then((byAgent) => {
      if (live && byAgent) bump((n) => n + 1);
    });
    return () => {
      live = false;
    };
  }, [connection, link]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `landed` is the re-read trigger: a load has stored a newer catalog.
  return useMemo(() => modelsOf(link.hostKey), [link.hostKey, landed]);
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
