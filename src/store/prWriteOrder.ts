/** Write ordering for the PR store slices.
 *
 *  `prStates` / `prChecks` have several concurrent writers, and centralizing the
 *  polling into `gitSync` did not reduce them to one:
 *
 *    - the fleet sweep (`refreshAllPrStatus`, 20s) writes *every* agent's keys,
 *    - the focused-agent poller (`fetchPrLive`, 5s) writes the selected one's,
 *    - `createPr` / `commitAndOpenPr` and the pushed `pr:state_changed` event
 *      write authoritatively at arbitrary moments.
 *
 *  The first two overlap on the focused agent's keys at different cadences, so
 *  an older request can still resolve *last* and overwrite newer data. That
 *  regressed the UI: a merged badge flipping back to open because a request
 *  issued before the merge landed after it, or a stale CI tint persisting until
 *  the next tick.
 *
 *  Every write claims a ticket from one monotonic counter *before* its request,
 *  then applies only if no later-issued write has already landed for the same
 *  slice + key.
 *
 *  A consequence worth naming: a response that was issued earlier but observed
 *  fresher data is discarded rather than reordered — there is no server-side
 *  freshness signal to compare, and issue order is the only total order we own.
 *  The next tick re-reads it, so the effect is at most one cadence of staleness,
 *  never a regression. */

export type PrSlice = "prStates" | "prChecks" | "prComments";

let ticket = 0;

/** Highest ticket applied, per slice then per store key. Nested rather than a
 *  composite string key so no separator can collide with an agent id or subdir. */
const applied: Record<PrSlice, Map<string, number>> = {
  prStates: new Map(),
  prChecks: new Map(),
  prComments: new Map(),
};

/** Claim an issue order. Call once per write, *before* awaiting the request. */
export const issuePrWrite = (): number => ++ticket;

/** True if `issued` is still the newest write to land for `slice`/`key`. Records
 *  it as applied, so re-checking the same ticket reports false the second time —
 *  call it once per (slice, key) at the point of writing. */
export const acceptPrWrite = (slice: PrSlice, key: string, issued: number): boolean => {
  const seen = applied[slice];
  if (issued <= (seen.get(key) ?? 0)) return false;
  seen.set(key, issued);
  return true;
};

/** Record an authoritative, synchronous write as the newest for `slice`/`key` —
 *  a mutation result (`createPr`) or a state change the backend pushed to us.
 *
 *  These awaited nothing of their own, so they need no ticket to protect them
 *  from each other. They must still *advance* the counter: an unstamped write
 *  leaves the high-water mark where it was, so a poll already in flight — one
 *  that observed the world before the PR existed, or before it merged — would
 *  still be accepted afterwards and overwrite it. */
export const stampPrWrite = (slice: PrSlice, key: string): void => {
  applied[slice].set(key, issuePrWrite());
};

/** Tests only — the counter and applied maps are module-global. */
export const resetPrWriteOrder = (): void => {
  ticket = 0;
  for (const seen of Object.values(applied)) seen.clear();
};
