/** Write ordering for the PR store slices.
 *
 *  The PR slices have several concurrent writers:
 *
 *    - the fleet seed (`loadAllPrStatus`, on connect / switch / focus) writes
 *      *every* agent's keys,
 *    - the focused checkouts' one-shot reads (`fetchPrLive`, `fetchPrThreads`)
 *      write the selected agent's,
 *    - `createPr` / `commitAndOpenPr` and the host watcher's pushed
 *      `pr:state_changed` / `pr:checks_changed` / `pr:threads_changed` write
 *      authoritatively at arbitrary moments.
 *
 *  The reads overlap on the focused agent's keys, so an older request can still
 *  resolve *last* and overwrite newer data. That regressed the UI: a merged
 *  badge flipping back to open because a request issued before the merge
 *  landed after it, or a stale CI tint persisting until the next event.
 *
 *  Every write claims a ticket from one monotonic counter *before* its request,
 *  then applies only if no later-issued write has already landed for the same
 *  slice + key.
 *
 *  A consequence worth naming: a response that was issued earlier but observed
 *  fresher data is discarded rather than reordered — there is no server-side
 *  freshness signal to compare, and issue order is the only total order we own.
 *  The watcher's next event (or the next resync) carries it, so the effect is
 *  brief staleness, never a regression. */

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
