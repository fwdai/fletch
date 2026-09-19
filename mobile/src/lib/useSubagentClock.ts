import type { BackgroundTaskMap } from "@desktop/adapters/shared/backgroundTasks";
import { useTick } from "./hooks";
import { subagentCounts, subagentTickMs } from "./subagents";

/** `Date.now()` and the sub-agent counts for a surface that shows them, kept
 *  fresh at the rate the screen needs (see subagentTickMs). One clock for the
 *  list row and the chat strip, so a failure expires the same way on both —
 *  and a surface with nothing to show runs no timer. `live` asks for the
 *  per-second rate regardless, for a surface with its own elapsed timer. */
export function useSubagentClock(tasks: BackgroundTaskMap | undefined, live = false) {
  const now = Date.now();
  const counts = subagentCounts(tasks, now);
  const ms = subagentTickMs(counts, live);
  useTick(ms ?? 1_000, ms !== undefined);
  return { now, counts };
}
