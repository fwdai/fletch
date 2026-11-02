// Coding-agent providers shown in the composer's model picker.
//
// `ProviderId` is the single source of truth for the set of agents. Every
// keyed registry (ADAPTERS, PROVIDER_DETAIL, PROVIDER_COMMANDS) is typed
// against it, so adding/removing an agent surfaces as a compile error
// anywhere it isn't kept in sync.
//
// Every agent here is wired: it has an entry in the frontend `ADAPTERS`
// registry (a full Record<ProviderId, …>) plus a backend runner, so a
// ProviderId without an adapter is a compile error.

export type ProviderId = "claude" | "codex" | "cursor" | "antigravity" | "opencode" | "pi";

export interface Provider {
  id: ProviderId;
  label: string;
  short: string;
  hue: number;
  /** Agent manages its own model and ignores a per-session selection, so the
   *  picker offers no model choice (e.g. antigravity's `agy --print` runner). */
  fixedModel?: boolean;
  /** Fletch can write this provider's session files, so a new session can
   *  continue a conversation natively: a fork's full conversation, a rewind
   *  that resumes rather than summarizes. Providers without it carry a
   *  conversation over as a summary only. Mirrors the backend capability
   *  `AgentCapabilities::transcript_writer`. */
  transcriptWriter?: boolean;
}

// Versions and per-account status are never hardcoded here: the backend probes
// real versions into the store (`providerVersions`, see refreshProviderVersions),
// and honest, non-user-specific model routing lives in PROVIDER_DETAIL.
export const PROVIDERS: Provider[] = [
  { id: "claude", label: "Claude Code", short: "CC", hue: 28, transcriptWriter: true },
  { id: "codex", label: "Codex", short: "CX", hue: 145, transcriptWriter: true },
  { id: "cursor", label: "Cursor Agent", short: "CR", hue: 215 },
  { id: "antigravity", label: "Antigravity", short: "AG", hue: 260, fixedModel: true },
  { id: "opencode", label: "OpenCode", short: "OC", hue: 195 },
  { id: "pi", label: "Pi", short: "PI", hue: 320, transcriptWriter: true },
];

export const DEFAULT_PROVIDER_ID = "claude";

/** Whether a new session of provider `id` can continue a conversation
 *  natively (`Provider.transcriptWriter`). Unknown ids can't. */
export function hasTranscriptWriter(id: string | null | undefined): boolean {
  return !!PROVIDERS.find((p) => p.id === id)?.transcriptWriter;
}

/** Which MCP transports each provider can attach — mirrors the backend
 *  delivery builders in `agent_profile.rs`, one entry per wired
 *  `McpDeliveryBuilder`:
 *    - claude   — stdio + http via `--mcp-config`
 *    - codex    — stdio only via `-c mcp_servers.*` (its `-c` config has no
 *                 http transport)
 *    - opencode — stdio + http, via an `OPENCODE_CONFIG` overlay
 *  cursor, pi and antigravity are absent on purpose. pi and agy speak no MCP
 *  at all (pi ships an extension system, agy only plugins); cursor's only
 *  config surface is ambient and agent-writable, so it needs sandbox work
 *  before it can be delivered safely. Unknown ids are treated as unsupported. */
export type McpSupport = "all" | "stdio" | "none";
export const MCP_SUPPORT: Record<string, McpSupport> = {
  claude: "all",
  codex: "stdio",
  opencode: "all",
};

/** Providers that can actually receive an MCP server, as human labels — the
 *  same set the backend derives from `agent::mcp_delivery`. Code indexing is
 *  delivered to agents over MCP, so this doubles as "which agents benefit from
 *  the code-indexing setting". Derived rather than hardcoded so wiring a new
 *  provider updates the settings copy for free. */
export function mcpCapableLabels(): string[] {
  return PROVIDERS.filter((p) => (MCP_SUPPORT[p.id] ?? "none") !== "none").map((p) => p.label);
}

/** Whether a provider with `support` can actually deliver an MCP server of
 *  `transport` at spawn. The single rule behind the agent editor's disabled
 *  rows, its save-path filter, and the spawn snapshot in `store/drafts.ts`,
 *  so the three can't drift. */
export function mcpAttachable(support: McpSupport, transport: "stdio" | "http"): boolean {
  return support === "all" || (support === "stdio" && transport !== "http");
}

/** URL for a provider/agent's brand icon on the website CDN. Icons live at
 *  /agents/<slug>.svg (slug === provider id), so a rebrand only needs the SVG
 *  re-uploaded — no app release. The webview's disk cache serves it offline
 *  after the first load; consumers fall back to the abbreviation monogram when
 *  the image is missing or hasn't loaded yet. */
export const agentIconUrl = (slug: string) => `https://fletch.sh/agents/${slug}.svg`;

/** Human-readable name for a provider id (e.g. "claude" → "Claude Code").
 *  Falls back to the raw id when unknown so we never render an empty label. */
export function providerLabel(id: string | null | undefined): string {
  if (!id) return PROVIDERS.find((p) => p.id === DEFAULT_PROVIDER_ID)?.label ?? DEFAULT_PROVIDER_ID;
  return PROVIDERS.find((p) => p.id === id)?.label ?? id;
}

/** Chip metadata (monogram + hue) for a provider id, used to render a brand
 *  icon via {@link ProviderIcon}. Mirrors {@link providerLabel}'s fallbacks so
 *  the chip and its tooltip never disagree: null/undefined resolves to the
 *  default provider, a known id to itself, and any unknown id to a derived
 *  two-letter monogram with neutral hue (matching the raw-id label). */
export function providerChip(id: string | null | undefined): { short: string; hue: number } {
  const p = PROVIDERS.find((x) => x.id === (id || DEFAULT_PROVIDER_ID));
  if (p) return { short: p.short, hue: p.hue };
  return { short: (id ?? "").slice(0, 2).toUpperCase(), hue: 0 };
}

export interface Accent {
  id: string;
  label: string;
  color: string;
}

export const ACCENTS: Accent[] = [
  { id: "copper", label: "Copper", color: "oklch(0.72 0.13 50)" },
  { id: "rust", label: "Rust", color: "oklch(0.6 0.16 30)" },
  { id: "olive", label: "Olive", color: "oklch(0.68 0.11 110)" },
  { id: "sage", label: "Sage", color: "oklch(0.7 0.09 160)" },
  { id: "ocean", label: "Ocean", color: "oklch(0.68 0.12 220)" },
  { id: "plum", label: "Plum", color: "oklch(0.65 0.13 320)" },
];

interface AccentValues {
  accent: string;
  soft: string;
  line: string;
}
export const ACCENT_VALUES: Record<string, AccentValues> = {
  copper: {
    accent: "oklch(0.72 0.13 50)",
    soft: "oklch(0.72 0.13 50 / .14)",
    line: "oklch(0.72 0.13 50 / .35)",
  },
  rust: {
    accent: "oklch(0.6 0.16 30)",
    soft: "oklch(0.6 0.16 30 / .14)",
    line: "oklch(0.6 0.16 30 / .35)",
  },
  olive: {
    accent: "oklch(0.68 0.11 110)",
    soft: "oklch(0.68 0.11 110 / .14)",
    line: "oklch(0.68 0.11 110 / .35)",
  },
  sage: {
    accent: "oklch(0.7 0.09 160)",
    soft: "oklch(0.7 0.09 160 / .14)",
    line: "oklch(0.7 0.09 160 / .35)",
  },
  ocean: {
    accent: "oklch(0.68 0.12 220)",
    soft: "oklch(0.68 0.12 220 / .14)",
    line: "oklch(0.68 0.12 220 / .35)",
  },
  plum: {
    accent: "oklch(0.65 0.13 320)",
    soft: "oklch(0.65 0.13 320 / .14)",
    line: "oklch(0.65 0.13 320 / .35)",
  },
};
