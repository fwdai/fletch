// Static per-provider metadata for the Providers settings pane. Keyed by the
// `PROVIDERS` id in data/providers.ts.
//
// `path` is the fallback binary location shown before the live probe resolves;
// `models` describes the tool's model routing (not user-specific).
// `installed: false` keeps a provider out of the "Installed" list.

import type { ProviderId } from "./providers";

export interface ThinkingLevel {
  label: string;
  /** Raw value passed verbatim to the provider CLI flag. */
  value: string;
}

export interface ProviderDetail {
  /** Fallback binary path, shown before the live probe resolves. */
  path: string;
  /** Models exposed by this agent. */
  models: string;
  /** Detected & configured on this machine — drives the "Installed" list. */
  installed: boolean;
  /** Thinking/reasoning effort levels supported by this provider's CLI.
   *  Empty means the provider has no effort flag — the picker hides. */
  thinkingLevels: ThinkingLevel[];
  /** Preferred initial level. Falls back to the highest level when unset. */
  defaultLevel?: string;
  /** True when effort is a session-level spawn flag (claude `--effort`) rather
   *  than a per-message arg. The composer hides the picker on an existing
   *  session, since the value can't change mid-session without restarting the
   *  process (which would discard the conversation's prompt cache). */
  effortAtSpawn?: boolean;
}

export const PROVIDER_DETAIL: Record<ProviderId, ProviderDetail> = {
  claude: {
    path: "/opt/homebrew/bin/claude",
    models: "Opus 4.7 · Sonnet 4.6 · Haiku 4",
    installed: true,
    // `claude --effort <level>` is a session-level spawn flag (not per-message):
    // chosen at session creation, threaded through spawn_agent, and persisted on
    // the session record so it re-applies on every spawn. Fixed for the session.
    thinkingLevels: [
      { label: "Low",   value: "low"    },
      { label: "Med",   value: "medium" },
      { label: "High",  value: "high"   },
      { label: "xHigh", value: "xhigh"  },
      { label: "Max",   value: "max"    },
    ],
    defaultLevel: "xhigh", // matches Claude Code's own default
    effortAtSpawn: true,
  },
  codex: {
    path: "~/.codex/bin/codex",
    models: "GPT-5.2-codex · o4-mini",
    installed: true,
    // `codex exec -c reasoning_effort="<value>"`
    thinkingLevels: [
      { label: "Low",  value: "low"    },
      { label: "Med",  value: "medium" },
      { label: "High", value: "high"   },
    ],
  },
  cursor: {
    path: "/Applications/Cursor.app/…/cursor-agent",
    models: "Composer · Auto",
    installed: true,
    // Cursor encodes effort in model names — no standalone flag.
    thinkingLevels: [],
  },
  antigravity: {
    path: "/Applications/Antigravity.app/…/antigravity",
    models: "Gemini 3 Pro · Flash",
    // No adapter/runner yet — kept out of the Installed list and gated as
    // "coming soon" in the picker. Flip to true once it's wired end-to-end.
    installed: false,
    thinkingLevels: [],
  },
  opencode: {
    path: "~/.opencode/bin/opencode",
    models: "Routed via upstream provider",
    installed: true,
    // `opencode run --variant <value>`
    thinkingLevels: [
      { label: "Low",  value: "minimal" },
      { label: "High", value: "high"    },
      { label: "Max",  value: "max"     },
    ],
  },
  pi: {
    path: "~/.pi/bin/pi",
    models: "Pi · experimental",
    installed: true,
    // `pi --thinking <value>`
    thinkingLevels: [
      { label: "Off",   value: "off"    },
      { label: "Low",   value: "low"    },
      { label: "Med",   value: "medium" },
      { label: "High",  value: "high"   },
      { label: "xHigh", value: "xhigh" },
    ],
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
