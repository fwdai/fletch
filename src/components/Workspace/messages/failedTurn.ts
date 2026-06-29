import { APP_ACTION_PREFIX } from "../../RightPanel/delegation";
import type { ViewItem } from "./pair";

/** Index of the user bubble whose turn failed (so the chat can offer a Retry on
 *  it), or -1 when no turn is in a failed state. Two failure shapes:
 *
 *   A) a send that threw before the turn started — flagged on the optimistic
 *      `user_message` itself (`failed`);
 *   B) the latest user turn whose response ended in an error notice.
 *
 *  Returns -1 while the agent is busy or the transcript is loading, so a retry
 *  never shows mid-flight. Anchored on explicit failure signals rather than
 *  "no agent reply yet", which would false-positive on turns that legitimately
 *  end without a final assistant message (e.g. a tool-only turn). */
export function failedTurnIndex(
  items: ViewItem[],
  { busy, loading }: { busy: boolean; loading: boolean },
): number {
  if (busy || loading) return -1;

  // A) A flagged failed send wins — it's the bubble the user just tried to send.
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === "user_message" && it.failed) return i;
  }

  // B) Otherwise, the latest real user turn (git-action chips excluded) counts
  // as failed when an error notice landed after it.
  let lastUserIdx = -1;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === "user_message" && !it.text.startsWith(APP_ACTION_PREFIX)) {
      lastUserIdx = i;
      break;
    }
  }
  if (lastUserIdx === -1) return -1;

  const errored = items
    .slice(lastUserIdx + 1)
    .some((it) => it.kind === "notice" && (it.is_error || it.subtype === "error"));
  return errored ? lastUserIdx : -1;
}
