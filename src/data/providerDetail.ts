// Enriched, presentational metadata for the Providers settings pane, keyed by
// the `PROVIDERS` id in data/providers.ts. This is mock data: real detection /
// authentication isn't wired yet, so these describe how a configured agent
// *would* present. The enable toggle still drives the real `providerFlags`.
//
// Typed as a full Record<ProviderId, …> so every provider must have an entry;
// `installed: false` keeps a provider out of the "Installed" list (e.g.
// antigravity, which has no adapter/runner yet).

import type { ProviderId } from "./providers";

export interface ProviderDetail {
  /** Account the agent is authenticated as. */
  account: string;
  /** Subscription / plan line. */
  plan: string;
  /** Resolved binary path. */
  path: string;
  /** Models exposed by this agent. */
  models: string;
  /** Available update version, if any (shows an update affordance). */
  update?: string;
  /** Render an "Early Access" badge. */
  earlyAccess?: boolean;
  /** Detected & configured on this machine — drives the "Installed" list. */
  installed: boolean;
}

export const PROVIDER_DETAIL: Record<ProviderId, ProviderDetail> = {
  claude: {
    account: "alex@joineve.ai",
    plan: "Claude Team",
    path: "/opt/homebrew/bin/claude",
    models: "Opus 4.7 · Sonnet 4.6 · Haiku 4",
    update: "v1.0.43",
    installed: true,
  },
  codex: {
    account: "alex@joineve.ai",
    plan: "ChatGPT Plus",
    path: "~/.codex/bin/codex",
    models: "GPT-5.2-codex · o4-mini",
    installed: true,
  },
  cursor: {
    account: "alex@joineve.ai",
    plan: "Cursor Pro",
    path: "/Applications/Cursor.app/…/cursor-agent",
    models: "Composer · Auto",
    earlyAccess: true,
    installed: true,
  },
  antigravity: {
    account: "alex@joineve.ai",
    plan: "Google AI Pro",
    path: "/Applications/Antigravity.app/…/antigravity",
    models: "Gemini 3 Pro · Flash",
    // No adapter/runner yet — kept out of the Installed list and gated as
    // "coming soon" in the picker. Flip to true once it's wired end-to-end.
    installed: false,
  },
  opencode: {
    account: "opencode",
    plan: "1 upstream connected",
    path: "~/.opencode/bin/opencode",
    models: "Routed via upstream provider",
    installed: true,
  },
  pi: {
    account: "pi",
    plan: "Experimental",
    path: "~/.pi/bin/pi",
    models: "Pi · experimental",
    earlyAccess: true,
    installed: true,
  },
};

export interface AvailableAgent {
  id: string;
  label: string;
  short: string;
  hue: number;
  version: string | null;
  /** "detected" = found on PATH but unconfigured; "install" = installable;
   *  "soon" = not yet supported in Quorum (no adapter/runner). */
  state: "detected" | "install" | "soon";
  note: string;
}

// Agents found on PATH but not yet configured, plus ones available to install.
export const AVAILABLE_AGENTS: AvailableAgent[] = [
  {
    id: "antigravity",
    label: "Antigravity",
    short: "AG",
    hue: 260,
    version: "v1.0",
    state: "soon",
    note: "Not yet supported in Quorum.",
  },
];
