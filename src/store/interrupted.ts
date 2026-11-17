import { perHost } from "./perHost";

/** Each host's agents the user stopped, whose dying process may still flush a
 *  turn_end: that one must not chime as a completion. */
export const interruptedAgents = perHost(() => new Set<string>());

/** Each host's agents whose current turn already signalled an error. One
 *  failure can arrive twice — a failed turn_end, then the process exiting into
 *  `error` status — so whichever lands first signals and the other is skipped.
 *  Cleared on `running` and `spawning` — the start of a new turn or spawn
 *  attempt. */
export const erroredAgents = perHost(() => new Set<string>());
