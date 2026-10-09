import type { ViewItem } from "../messages/pair";

// The vendors' own wording, which neither documents or keeps stable. Claude:
// "API Error: 401 … OAuth access token has expired … Please run /login",
// "Claude AI usage limit reached|…", "5-hour limit reached ∙ resets 3pm",
// "You've hit your limit · resets …", "You've hit your session limit · resets
// 1pm (Asia/Bangkok)", "Invalid API key · Please run /login".
// Codex: "You've hit your usage limit", relayed "401 Unauthorized" / "429 Too
// Many Requests" bodies, "could not be refreshed", "Please sign in again". Kept
// to phrases an agent's own prose about auth code or rate limiters won't hit;
// a miss only costs the hint, the header picker is always there.
const REPLY_ERROR =
  /API Error: 4(01|29)\b|run \/login\b|limit reached|hit your (\w+ )?limit|token has expired|401 Unauthorized|429 Too Many Requests|could not be refreshed|please sign in again|not signed in|not logged in|invalid api key|credit balance is too low/i;

// An error notice is the vendor speaking, never the agent, so codex's bare
// "usage limit" and a bare "/login" are safe to read there.
const NOTICE_ERROR = new RegExp(`${REPLY_ERROR.source}|usage limit|/login\\b`, "i");

/** Whether the latest turn (everything after the last user message) failed on
 *  something another account could fix: a limit or a sign-in problem. Reads
 *  the turn's last error notice and the agent message just before it: claude
 *  shrinks the notice to "Turn failed" once the agent has spoken, leaving the
 *  vendor's text in that reply. */
export function endedOnAccountError(items: readonly ViewItem[]): boolean {
  let i = items.length - 1;
  for (; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === "user_message") return false;
    if (it.kind === "notice" && (it.subtype === "error" || it.is_error)) break;
  }
  if (i < 0) return false;
  const notice = items[i];
  if (notice.kind === "notice" && NOTICE_ERROR.test(notice.text)) return true;
  for (let j = i - 1; j >= 0; j -= 1) {
    const it = items[j];
    if (it.kind === "user_message") return false;
    if (it.kind === "agent_message") return REPLY_ERROR.test(it.text);
  }
  return false;
}
