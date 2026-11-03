// The words the run rail shows for the agent chain — provider › account ›
// model. The rail is the composer's one line about how the agent runs, so
// every piece is as short as it can be and still be read: the provider is its
// icon, the account its login's first word, the model its family and version.
// The full names live in the pieces' tooltips and in the menus they open.

/** A model's name without the vendor the provider icon already states:
 *  `Claude Fable 5.1` → `Fable 5.1`, `OpenAI o4-mini` → `o4-mini`. `GPT-5.5
 *  Codex` keeps its prefix: GPT is the family, not the vendor. Anything else
 *  is left alone. */
export function shortModelName(name: string): string {
  return name.replace(/^(?:Claude|Anthropic|OpenAI)\s+/i, "").trim() || name;
}

/** How long an account name may run in the rail before it is cut. */
const ACCOUNT_NAME_MAX = 14;

/** An account's name as the rail shows it: an email's local part, a long
 *  name cut with an ellipsis. `Terminal login` (the default account) reads
 *  as `Terminal`. The full name is the piece's tooltip. */
export function compactAccountName(label: string): string {
  const trimmed = label.trim();
  if (trimmed === "Terminal login") return "Terminal";
  const at = trimmed.indexOf("@");
  const base = at > 0 ? trimmed.slice(0, at) : trimmed;
  return base.length > ACCOUNT_NAME_MAX
    ? `${base.slice(0, ACCOUNT_NAME_MAX - 1).trimEnd()}…`
    : base;
}

/** The pieces of a tooltip joined the way every rail tip reads:
 *  `Claude Code · alex · Claude Fable 5.1`. Empty pieces are skipped. */
export function chainTip(...parts: (string | null | undefined | false)[]): string {
  return parts.filter((p): p is string => typeof p === "string" && p.length > 0).join(" · ");
}
