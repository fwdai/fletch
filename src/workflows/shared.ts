// shared.ts — agent resolution + builder helpers.
//
// A workflow step references either a custom agent (by id) or a base provider
// (by id). resolveAgent collapses both into one display identity the builder
// renders, reusing the host's real custom-agent and provider data.

import type { ModelMeta } from "../data/modelCatalog/types";
import { PROVIDERS } from "../data/providers";
import type { CustomAgent } from "../storage/customAgents";
import type { AgentSpec } from "./spec";

/** Two-letter monogram from a name's initials, falling back to a neutral dot. */
export function shortFor(name: string): string {
  const initials = (name || "")
    .trim()
    .split(/\s+/)
    .map((w) => w[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();
  return initials || "·";
}

export interface ResolvedAgent {
  name: string;
  short: string;
  hue: number;
  model: string | null;
  /** Base provider label for a custom agent; null for a base provider itself. */
  baseLabel: string | null;
  /** Underlying provider slug — the provider id for a base agent, or a custom
   *  agent's base. Used to render the provider brand icon. */
  providerId: string;
  custom: boolean;
}

/** A provider's default model id, used when a step picks a base provider or a
 *  custom agent left its model on "provider default". */
export function defaultModel(
  providerId: string,
  modelsByAgent: Record<string, ModelMeta[]>,
): string | null {
  return modelsByAgent[providerId]?.[0]?.id ?? null;
}

export function resolveAgent(
  agentId: string | null,
  agents: CustomAgent[],
  modelsByAgent: Record<string, ModelMeta[]>,
): ResolvedAgent | null {
  if (!agentId) return null;
  const ca = agents.find((a) => a.id === agentId);
  if (ca) {
    const prov = PROVIDERS.find((p) => p.id === ca.base);
    return {
      name: ca.name,
      short: shortFor(ca.name),
      hue: ca.color,
      model: ca.model ?? defaultModel(ca.base, modelsByAgent),
      baseLabel: prov?.label ?? null,
      providerId: ca.base,
      custom: true,
    };
  }
  const p = PROVIDERS.find((x) => x.id === agentId);
  if (p) {
    return {
      name: p.label,
      short: p.short,
      hue: p.hue,
      model: defaultModel(p.id, modelsByAgent),
      baseLabel: null,
      providerId: p.id,
      custom: false,
    };
  }
  return null;
}

/** Resolve a spec agent *alias* to its display identity: look the alias up in
 *  the spec's `agents` map, then resolve the picked custom-agent id or base
 *  provider — falling back to the alias itself (which for a base provider equals
 *  its id). Returns `null` for a missing alias. */
export function resolveAlias(
  agents: Record<string, AgentSpec> | undefined,
  alias: string | undefined,
  customAgents: CustomAgent[],
  modelsByAgent: Record<string, ModelMeta[]>,
): ResolvedAgent | null {
  if (!alias) return null;
  const a = agents?.[alias];
  return resolveAgent(a?.custom_agent ?? a?.base ?? alias, customAgents, modelsByAgent);
}

/** A resolver bound to the current agent/model data — handy to pass to children
 *  so they don't each thread both arguments through. */
export type AgentResolver = (agentId: string | null) => ResolvedAgent | null;

/** A filesystem/alias-friendly slug of `s`, or `fallback` when it reduces to
 *  nothing: lowercases, collapses runs of non-alphanumerics to `-`, trims dashes. */
export function slugify(s: string, fallback: string): string {
  return (
    s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || fallback
  );
}
