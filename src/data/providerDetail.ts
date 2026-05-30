// Enriched, presentational metadata for the Providers settings pane, keyed by
// the `PROVIDERS` id in data/providers.ts. This is mock data: real detection /
// authentication isn't wired yet, so these describe how a configured agent
// *would* present. The enable toggle still drives the real `providerFlags`.

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

export const PROVIDER_DETAIL: Record<string, ProviderDetail> = {
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
  gemini: {
    account: "alex@joineve.ai",
    plan: "Gemini Advanced",
    path: "~/.gemini/bin/gemini",
    models: "Gemini 2.5 Pro · Flash",
    installed: true,
  },
  opencode: {
    account: "opencode",
    plan: "1 upstream connected",
    path: "~/.opencode/bin/opencode",
    models: "Routed via upstream provider",
    installed: true,
  },
};

export interface AvailableAgent {
  id: string;
  label: string;
  short: string;
  hue: number;
  version: string | null;
  /** "detected" = found on PATH but unconfigured; "install" = installable. */
  state: "detected" | "install";
  note: string;
}

// Agents found on PATH but not yet configured, plus ones available to install.
export const AVAILABLE_AGENTS: AvailableAgent[] = [
  {
    id: "pi",
    label: "Pi Coder",
    short: "PI",
    hue: 320,
    version: "v0.4",
    state: "detected",
    note: "Detected on PATH — not configured yet.",
  },
  {
    id: "aider",
    label: "Aider",
    short: "AI",
    hue: 95,
    version: "v0.71.1",
    state: "detected",
    note: "Detected on PATH — not configured yet.",
  },
  {
    id: "amp",
    label: "Amp",
    short: "AM",
    hue: 175,
    version: null,
    state: "install",
    note: "Sourcegraph's agentic coding CLI.",
  },
];
