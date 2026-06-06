// Turns a raw model id from an agent transcript (e.g. `claude-opus-4-7`,
// `gpt-5.2-codex`, `gemini-3-pro`) into a human label (`Claude Opus 4.7`,
// `GPT-5.2 Codex`, `Gemini 3 Pro`) for display next to the provider.
//
// Best-effort: known families (Claude, GPT, OpenAI o-series, Gemini, Grok) get
// a tailored label; anything unrecognized (OpenCode can route arbitrary
// upstream models) is returned unchanged rather than mangled.

function cap(word: string): string {
  return word ? word[0].toUpperCase() + word.slice(1) : word;
}

/** Capitalize each dash-separated token: `flash-free` → `Flash Free`. */
function titleizeTokens(s: string): string {
  return s.split("-").filter(Boolean).map(cap).join(" ");
}

export function prettyModelLabel(id: string): string {
  const raw = id.trim();
  if (!raw) return raw;

  // Drop a trailing date snapshot (`claude-haiku-4-5-20251001`).
  const base = raw.replace(/-\d{8}$/, "");

  // Claude: claude-<tier>-<major>-<minor> → "Claude Opus 4.7".
  let m = base.match(/^claude-(opus|sonnet|haiku)-(\d+)-(\d+)$/);
  if (m) return `Claude ${cap(m[1])} ${m[2]}.${m[3]}`;
  // Claude without a minor: claude-opus-4 → "Claude Opus 4".
  m = base.match(/^claude-(opus|sonnet|haiku)-(\d+)$/);
  if (m) return `Claude ${cap(m[1])} ${m[2]}`;

  // GPT: gpt-5.2-codex → "GPT-5.2 Codex"; gpt-5.5 → "GPT-5.5".
  m = base.match(/^gpt-([\d.]+)(?:-(.+))?$/);
  if (m) return `GPT-${m[1]}${m[2] ? ` ${titleizeTokens(m[2])}` : ""}`;

  // OpenAI o-series keeps its lowercase-o convention: o4-mini → "o4-mini".
  if (/^o\d/.test(base)) return base;

  // Gemini: gemini-3-pro → "Gemini 3 Pro".
  m = base.match(/^gemini-(.+)$/);
  if (m) return `Gemini ${titleizeTokens(m[1])}`;

  // Grok: grok-code → "Grok Code".
  m = base.match(/^grok-(.+)$/);
  if (m) return `Grok ${titleizeTokens(m[1])}`;

  // Unknown shape (e.g. a routed upstream model) — leave as-is.
  return raw;
}
