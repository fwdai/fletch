import type { ViewItem } from "../messages/pair";

// Matched against the provider's own wording, which neither vendor documents or
// keeps stable: claude's "usage limit reached", "You've hit your limit",
// "Invalid API key · Please run /login", "OAuth token has expired"; codex's
// "You've hit your usage limit", relayed 401/429 bodies, "log in again". A
// miss only costs the hint; the header picker is always there.
const ACCOUNT_ERROR =
  /usage limit|rate.?limit|limit reached|hit your( usage)? limit|quota|too many requests|\b429\b|\b401\b|unauthori[sz]ed|authenticat|api key|\/login|log ?in again|not logged in|token (has )?expired|could not be refreshed|credit balance/i;

/** Whether the latest turn (everything after the last user message) failed on
 *  something another account could fix: a limit or a sign-in problem. Claude
 *  puts the detail in the error notice, or in the agent's reply when it had
 *  already spoken, so both are read. */
export function endedOnAccountError(items: readonly ViewItem[]): boolean {
  let failed = false;
  let matched = false;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === "user_message") break;
    if (it.kind === "notice" && (it.subtype === "error" || it.is_error)) {
      failed = true;
      if (ACCOUNT_ERROR.test(it.text)) matched = true;
    } else if (it.kind === "agent_message" && ACCOUNT_ERROR.test(it.text)) {
      matched = true;
    }
  }
  return failed && matched;
}
