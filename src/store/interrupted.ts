export const interruptedAgents = new Set<string>();

/** Agents whose current turn already signalled an error. One failure can arrive
 *  twice — a failed turn_end, then the process exiting into `error` status — so
 *  whichever lands first signals and the other is skipped. Cleared on `running`
 *  and `spawning` — the start of a new turn or spawn attempt. */
export const erroredAgents = new Set<string>();
