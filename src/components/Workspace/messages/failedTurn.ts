import { APP_ACTION_PREFIX } from "../../RightPanel/delegation";
import type { ViewItem } from "./pair";

/** Set of `items` indices whose user bubble should offer a Retry. Two failure
 *  shapes:
 *
 *   A) a send that threw before the turn started — flagged on the optimistic
 *      `user_message` itself (`failed`). Every such bubble is independently
 *      retryable, so two failed sends each keep their own button.
 *   B) the latest user turn whose response ended in an error notice.
 *
 *  Empty while the agent is busy or the transcript is loading, so a retry never
 *  shows mid-flight. Anchored on explicit failure signals rather than "no agent
 *  reply yet", which would false-positive on turns that legitimately end without
 *  a final assistant message (e.g. a tool-only turn). */
export function retryableTurns(
  items: ViewItem[],
  { busy, loading }: { busy: boolean; loading: boolean },
): Set<number> {
  const out = new Set<number>();
  if (busy || loading) return out;

  // A) Every flagged failed send is independently retryable.
  items.forEach((it, i) => {
    if (it.kind === "user_message" && it.failed) out.add(i);
  });

  // B) The latest real user turn (git-action chips excluded) counts as failed
  // when an error notice landed after it.
  let lastUserIdx = -1;
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const it = items[i];
    if (it.kind === "user_message" && !it.text.startsWith(APP_ACTION_PREFIX)) {
      lastUserIdx = i;
      break;
    }
  }
  if (lastUserIdx !== -1) {
    const errored = items
      .slice(lastUserIdx + 1)
      .some((it) => it.kind === "notice" && (it.is_error || it.subtype === "error"));
    if (errored) out.add(lastUserIdx);
  }

  return out;
}
