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
  { id: "claude",   label: "Claude Code",  short: "CC", version: "v1.0.42",     hue: 28,  sub: "Opus 4.7 · Sonnet 4.6" },
  { id: "codex",    label: "Codex",        short: "CX", version: "v0.133.0",    hue: 145, sub: "ChatGPT Plus" },
  { id: "cursor",   label: "Cursor Agent", short: "CR", version: "v2026.05.24", hue: 215, sub: "Pro Subscription" },
  { id: "gemini",   label: "Gemini CLI",   short: "GM", version: "v0.18",       hue: 260, sub: "Gemini 2.5 Pro" },
  { id: "opencode", label: "OpenCode",     short: "OC", version: "v1.15.10",    hue: 195, sub: "1 upstream connected" },
  { id: "pi",       label: "Pi Coder",     short: "PI", version: "v0.4",        hue: 320, sub: "Pi · experimental" },
];

export const DEFAULT_PROVIDER_ID = "claude";

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
