// Coding-agent providers. Only "claude" is actually wired; others are
// listed so the model picker has something to show. When real backends
// land, drop the mocks and add a `provider` field on AgentRecord.

export interface Provider {
  id: string;
  label: string;
  short: string;
  version: string;
  hue: number;
  sub: string;
}

export const PROVIDERS: Provider[] = [
  { id: "claude",      label: "Claude Code",  short: "CC", version: "v1.0.42",     hue: 28,  sub: "Opus 4.7 · Sonnet 4.6" },
  { id: "codex",       label: "Codex",        short: "CX", version: "v0.133.0",    hue: 145, sub: "ChatGPT Plus" },
  { id: "cursor",      label: "Cursor Agent", short: "CR", version: "v2026.05.24", hue: 215, sub: "Pro Subscription" },
  { id: "antigravity", label: "Antigravity",  short: "AG", version: "v1.0",        hue: 260, sub: "Gemini 3 Pro" },
  { id: "opencode",    label: "OpenCode",     short: "OC", version: "v1.15.12",    hue: 195, sub: "1 upstream connected" },
  { id: "pi",          label: "Pi Coder",     short: "PI", version: "v0.4",        hue: 320, sub: "Pi · experimental" },
];

export const DEFAULT_PROVIDER_ID = "claude";

/** URL for a provider/agent's brand icon on the website CDN. Icons live at
 *  /agents/<slug>.svg (slug === provider id), so a rebrand only needs the SVG
 *  re-uploaded — no app release. The webview's disk cache serves it offline
 *  after the first load; consumers fall back to the abbreviation monogram when
 *  the image is missing or hasn't loaded yet. */
export const agentIconUrl = (slug: string) =>
  `https://quorum.fwdai.org/agents/${slug}.svg`;

/** Human-readable name for a provider id (e.g. "claude" → "Claude Code").
 *  Falls back to the raw id when unknown so we never render an empty label. */
export function providerLabel(id: string | null | undefined): string {
  if (!id) return PROVIDERS.find((p) => p.id === DEFAULT_PROVIDER_ID)!.label;
  return PROVIDERS.find((p) => p.id === id)?.label ?? id;
}

export interface Accent {
  id: string;
  label: string;
  color: string;
}

export const ACCENTS: Accent[] = [
  { id: "copper", label: "Copper", color: "oklch(0.72 0.13 50)" },
  { id: "rust",   label: "Rust",   color: "oklch(0.6 0.16 30)" },
  { id: "olive",  label: "Olive",  color: "oklch(0.68 0.11 110)" },
  { id: "sage",   label: "Sage",   color: "oklch(0.7 0.09 160)" },
  { id: "ocean",  label: "Ocean",  color: "oklch(0.68 0.12 220)" },
  { id: "plum",   label: "Plum",   color: "oklch(0.65 0.13 320)" },
];

interface AccentValues {
  accent: string;
  soft: string;
  line: string;
}
export const ACCENT_VALUES: Record<string, AccentValues> = {
  copper: { accent: "oklch(0.72 0.13 50)",  soft: "oklch(0.72 0.13 50 / .14)",  line: "oklch(0.72 0.13 50 / .35)" },
  rust:   { accent: "oklch(0.6 0.16 30)",   soft: "oklch(0.6 0.16 30 / .14)",   line: "oklch(0.6 0.16 30 / .35)" },
  olive:  { accent: "oklch(0.68 0.11 110)", soft: "oklch(0.68 0.11 110 / .14)", line: "oklch(0.68 0.11 110 / .35)" },
  sage:   { accent: "oklch(0.7 0.09 160)",  soft: "oklch(0.7 0.09 160 / .14)",  line: "oklch(0.7 0.09 160 / .35)" },
  ocean:  { accent: "oklch(0.68 0.12 220)", soft: "oklch(0.68 0.12 220 / .14)", line: "oklch(0.68 0.12 220 / .35)" },
  plum:   { accent: "oklch(0.65 0.13 320)", soft: "oklch(0.65 0.13 320 / .14)", line: "oklch(0.65 0.13 320 / .35)" },
};
