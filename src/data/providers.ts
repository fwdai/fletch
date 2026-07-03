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
}

// Versions and per-account status are never hardcoded here: the backend probes
// real versions into the store (`providerVersions`, see refreshProviderVersions),
// and honest, non-user-specific model routing lives in PROVIDER_DETAIL.
export const PROVIDERS: Provider[] = [
  { id: "claude", label: "Claude Code", short: "CC", hue: 28 },
  { id: "codex", label: "Codex", short: "CX", hue: 145 },
  { id: "cursor", label: "Cursor Agent", short: "CR", hue: 215 },
  { id: "antigravity", label: "Antigravity", short: "AG", hue: 260, fixedModel: true },
  { id: "opencode", label: "OpenCode", short: "OC", hue: 195 },
  { id: "pi", label: "Pi", short: "PI", hue: 320 },
];

export const DEFAULT_PROVIDER_ID = "claude";

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
