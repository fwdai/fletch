import type { ViewItem } from "./pair";

/** True when the agent is busy but hasn't produced any output for the turn
 *  the user just started — the pre-output gap where an inline anchor helps. */
export function isTurnPending(items: ViewItem[]): boolean {
  if (items.length === 0) return false;
  const last = items[items.length - 1];
  // Only the prompt that *starts* a turn. Mid-turn queued follow-ups append
  // after ongoing agent activity and must not re-show the anchor.
  return (
    last.kind === "user_message" ||
    (last.kind === "notice" && last.subtype === "slash_command")
  );
}
