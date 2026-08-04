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
const SYSTEM_BLOCK = /\s*<(fletch|quorum)-system>[\s\S]*?<\/\1-system>\s*/g;

export function stripInjectedInstructions(text: string): string {
  return text.replace(SYSTEM_BLOCK, "").trim();
}

/** The first line of a turn Fletch authored itself rather than the user typing it.
 *
 *  Three producers write into a project's PM chat through the same
 *  `sendUserMessage` the composer uses — the settle review and mid-run awareness
 *  (`roadmap/review.rs`) and the standup digest (`Roadmap/Thread/standup.ts`) — so
 *  all three land as ordinary user-role turns. Unmarked, the user scrolling back
 *  reads Fletch's prompts as things they typed, and the fenced-data framing those
 *  prompts carry (run output is evidence, not instructions) is invisible to the one
 *  person who should see it.
 *
 *  The right shape is an `origin: user|system` column on the turn row; that is a
 *  chat schema change, so this is the documented interim: one line, in the same
 *  `<fletch-…>` family as the injected-instruction block above, stripped before
 *  display and rendered with a "Fletch · system" label instead of a user bubble
 *  (see `MessageItem`). It is content the agent sees too, so it says what it is
 *  rather than reading as noise — and the host declares the same literal
 *  (`roadmap/review.rs`'s `SYSTEM_TURN_MARKER`, pinned by a test there). */
export const SYSTEM_TURN_MARKER =
  "<fletch-system-turn>Fletch wrote this turn, not the user.</fletch-system-turn>";

/** The marker's tag, un-anchored for the same reason `SYSTEM_BLOCK` is: some
 *  agents nest the user message inside their own envelope, so our line can come
 *  back mid-string rather than at the start. Deliberately NOT folded into
 *  `SYSTEM_BLOCK` — that one is also applied at the data layer (`claude/sanitize`),
 *  which would erase the marker from stored history before the transcript could
 *  read it. */
const SYSTEM_TURN = /\s*<fletch-system-turn>[\s\S]*?<\/fletch-system-turn>\s*/g;

/** Did Fletch write this turn? Substring rather than `SYSTEM_TURN.test`, which
 *  would carry `lastIndex` state between calls. */
export function isSystemTurn(text: string): boolean {
  return text.includes("<fletch-system-turn>");
}

/** Strip the marker for display. No-op on turns without it. */
export function stripSystemTurnMarker(text: string): string {
  return text.replace(SYSTEM_TURN, "").trim();
}
