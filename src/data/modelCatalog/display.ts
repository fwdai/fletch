// Human-facing model names for chips and tooltips.
//
// The sidebar identity chip, Mission Control and the Roadmap thread all want
// the same one-liner — "Claude Code · Fable 5.1" — so the naming lives here
// once rather than per call-site.

import { effortLabel } from "../providerDetail";
import { lookupModel } from "./normalize";
import type { SlimCatalog } from "./types";

/** Short human name for a model id: the catalog's name ("Claude Fable 5.1"),
 *  else the raw id minus its provider prefix and bracket tag
 *  ("anthropic/claude-x[1m]" → "claude-x"). Null when there is no id. */
export function modelDisplayName(
  catalog: SlimCatalog,
  rawId: string | null | undefined,
): string | null {
  if (!rawId) return null;
  const meta = lookupModel(catalog, rawId);
  if (meta) return meta.name;
  const bare = rawId.includes("/") ? rawId.slice(rawId.lastIndexOf("/") + 1) : rawId;
  return bare.replace(/\[[^\]]*\]$/, "").trim() || null;
}

/** Drop a leading brand word the neighbouring label already carries, so
 *  "Claude Code · Claude Fable 5.1" reads "Claude Code · Fable 5.1". Only the
 *  first word is compared — "Cursor Agent" keeps "Claude Opus 4.5" intact. */
export function dropSharedBrand(name: string, label: string): string {
  const brand = label.split(" ")[0];
  if (!brand) return name;
  const prefix = `${brand} `;
  const stripped = name.startsWith(prefix) ? name.slice(prefix.length).trim() : name;
  return stripped || name;
}

/** The identity-chip tooltip: custom-agent name, provider, model, effort —
 *  whichever are known, joined with " · ". Model is the one chosen for the
 *  session, else the one the transcript last reported (a default-model
 *  session), else omitted. */
export function agentIdentityTip(opts: {
  providerLabel: string;
  customAgentName?: string | null;
  catalog: SlimCatalog;
  model?: string | null;
  /** The model the transcript last reported, when `model` is unset. */
  liveModel?: string | null;
  effort?: string | null;
}): string {
  const model = modelDisplayName(opts.catalog, opts.model ?? opts.liveModel);
  return [
    opts.customAgentName,
    opts.providerLabel,
    model && dropSharedBrand(model, opts.providerLabel),
    // The same label the thinking picker shows ("xhigh" → "xHigh").
    opts.effort && `${effortLabel(opts.effort)} effort`,
  ]
    .filter(Boolean)
    .join(" · ");
}
