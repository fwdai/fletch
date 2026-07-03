/** Strip the Fletch-injected instruction block from displayed user text.
 *
 *  The prepend-style agents (cursor, opencode, antigravity) receive the
 *  instructions as a `<fletch-system>…</fletch-system>` block prepended to the
 *  first user message, and echo it back into their transcript. This removes
 *  that block so the UI shows only what the user typed. The `<fletch-system>`
 *  tag is Fletch-specific, so removing it anywhere is safe — and it must be
 *  un-anchored because some agents (e.g. cursor) nest the user message inside
 *  their own envelope, leaving our block mid-string rather than at the start.
 *  The legacy `<quorum-system>` tag is still matched so transcripts recorded
 *  before the rebrand keep stripping cleanly. No-op on messages without it. */
const SYSTEM_BLOCK = /\s*<(?:fletch|quorum)-system>[\s\S]*?<\/(?:fletch|quorum)-system>\s*/g;

export function stripInjectedInstructions(text: string): string {
  return text.replace(SYSTEM_BLOCK, "").trim();
}
