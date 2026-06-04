// Static per-provider metadata for the Providers settings pane. Keyed by the
// `PROVIDERS` id in data/providers.ts.
//
// `path` is the fallback binary location shown before the live probe resolves;
// `models` describes the tool's model routing (not user-specific).
// `installed: false` keeps a provider out of the "Installed" list.

import type { ProviderId } from "./providers";

export interface ProviderDetail {
  /** Fallback binary path, shown before the live probe resolves. */
  path: string;
  /** Models exposed by this agent. */
  models: string;
  /** Detected & configured on this machine — drives the "Installed" list. */
  installed: boolean;
}

export const PROVIDER_DETAIL: Record<ProviderId, ProviderDetail> = {
  claude: {
    path: "/opt/homebrew/bin/claude",
    models: "Opus 4.7 · Sonnet 4.6 · Haiku 4",
    installed: true,
  },
  codex: {
    path: "~/.codex/bin/codex",
    models: "GPT-5.2-codex · o4-mini",
    installed: true,
  },
  cursor: {
    path: "/Applications/Cursor.app/…/cursor-agent",
    models: "Composer · Auto",
    installed: true,
  },
  antigravity: {
    path: "/Applications/Antigravity.app/…/antigravity",
    models: "Gemini 3 Pro · Flash",
    // No adapter/runner yet — kept out of the Installed list and gated as
    // "coming soon" in the picker. Flip to true once it's wired end-to-end.
    installed: false,
  },
  opencode: {
    path: "~/.opencode/bin/opencode",
    models: "Routed via upstream provider",
    installed: true,
  },
  pi: {
    path: "~/.pi/bin/pi",
    models: "Pi · experimental",
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
