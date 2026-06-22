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
  /** Copy-paste shell command to install the CLI, shown in the readiness
   *  check when the binary isn't found. Omit when there's no reliable
   *  one-liner — the UI falls back to the `docs` link. */
  install?: string;
  /** Setup / docs URL — always offered as "learn more" and the fallback when
   *  there's no `install` command. */
  docs: string;
  /** One-line hint for signing in after install. We detect the binary, not
   *  auth (which varies per CLI), so this nudges the user to complete it. */
  signIn?: string;
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
    install: "npm install -g @anthropic-ai/claude-code",
    docs: "https://docs.anthropic.com/en/docs/claude-code",
    signIn: "Run `claude` once in a terminal to sign in.",
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
    install: "npm install -g @openai/codex",
    docs: "https://github.com/openai/codex",
    signIn: "Run `codex` once in a terminal to sign in.",
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
    install: "curl https://cursor.com/install -fsS | bash",
    docs: "https://cursor.com",
    signIn: "Run `cursor-agent login` to sign in.",
    // Cursor encodes effort in model names — no standalone flag.
    thinkingLevels: [],
  },
  antigravity: {
    path: "/Applications/Antigravity.app/…/antigravity",
    models: "Gemini 3 Pro · Flash",
    installed: true,
    docs: "https://antigravity.google/product/antigravity-cli",
    thinkingLevels: [],
  },
  opencode: {
    path: "~/.opencode/bin/opencode",
    models: "Routed via upstream provider",
    installed: true,
    install: "curl -fsSL https://opencode.ai/install | bash",
    docs: "https://opencode.ai",
    signIn: "Run `opencode auth login` to connect a provider.",
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
    docs: "https://pi.dev/",
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
