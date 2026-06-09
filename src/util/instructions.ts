/** Strip the Quorum-injected instruction block from displayed user text.
 *
 *  The prepend-style agents (cursor, opencode, antigravity) receive the
 *  instructions as a `<quorum-system>…</quorum-system>` block prepended to the
 *  first user message, and echo it back into their transcript. This removes
 *  that block so the UI shows only what the user typed. The `<quorum-system>`
 *  tag is Quorum-specific, so removing it anywhere is safe — and it must be
 *  un-anchored because some agents (e.g. cursor) nest the user message inside
 *  their own envelope, leaving our block mid-string rather than at the start.
 *  No-op on messages that don't carry it. */
const QUORUM_SYSTEM_BLOCK = /\s*<quorum-system>[\s\S]*?<\/quorum-system>\s*/g;

export function stripInjectedInstructions(text: string): string {
  return text.replace(QUORUM_SYSTEM_BLOCK, "").trim();
}
