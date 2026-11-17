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
 *  brief staleness, never a regression.
 *
 *  The phone's store (mobile/src/store) follows the same watcher and orders its
 *  one-shot reads against its events through this module too.
 *
 *  One order covers one host's keys. The functions exported at the bottom are
 *  one shared order — this desktop's own engine's; the desktop gives each
 *  paired host another (`prWriteOrderOf` in ./prEvents), and the phone one per
 *  host store (mobile/src/store/host), since agent ids recur across hosts and
 *  one host's stamp must not refuse another's read. */

export type PrSlice = "prStates" | "prChecks" | "prComments" | "prSets";

export interface PrWriteOrder {
  /** Claim an issue order. Call once per write, *before* awaiting the request. */
  issue: () => number;
  /** True if `issued` is still the newest write to land for `slice`/`key`.
   *  Records it as applied, so re-checking the same ticket reports false the
   *  second time — call it once per (slice, key) at the point of writing. */
  accept: (slice: PrSlice, key: string, issued: number) => boolean;
  /** Record an authoritative, synchronous write as the newest for
   *  `slice`/`key` — a mutation result (`createPr`) or a state change the
   *  backend pushed to us.
   *
   *  These awaited nothing of their own, so they need no ticket to protect them
   *  from each other. They must still *advance* the counter: an unstamped write
   *  leaves the high-water mark where it was, so a poll already in flight — one
   *  that observed the world before the PR existed, or before it merged — would
   *  still be accepted afterwards and overwrite it. */
  stamp: (slice: PrSlice, key: string) => void;
  /** Refuse every write issued so far, for every key — for a store taking its
   *  host up afresh (the phone's, on every handshake and when it is switched
   *  back to), where an answer still in flight was asked of the socket before.
   *  Per-key marks cannot do this: a key never stamped since would take it.
   *
   *  Nothing issued afterwards is affected: the counter keeps rising, so the
   *  first read after the fence outranks it and every stamp left before it. */
  fence: () => void;
  /** Tests only: back to a fresh order. */
  reset: () => void;
}

export function createPrWriteOrder(): PrWriteOrder {
  let ticket = 0;

  /** Tickets at or below this were issued against a host the store has since
   *  left (see `fence`). */
  let floor = 0;

  /** Highest ticket applied, per slice then per store key. Nested rather than a
   *  composite string key so no separator can collide with an agent id or
   *  subdir. */
  const applied: Record<PrSlice, Map<string, number>> = {
    prStates: new Map(),
    prChecks: new Map(),
    prComments: new Map(),
    prSets: new Map(),
  };

  const issue = (): number => ++ticket;

  return {
    issue,
    accept: (slice, key, issued) => {
      if (issued <= floor) return false;
      const seen = applied[slice];
      if (issued <= (seen.get(key) ?? 0)) return false;
      seen.set(key, issued);
      return true;
    },
    stamp: (slice, key) => {
      applied[slice].set(key, issue());
    },
    fence: () => {
      floor = issue();
    },
    reset: () => {
      ticket = 0;
      floor = 0;
      for (const seen of Object.values(applied)) seen.clear();
    },
  };
}

/** The shared order: this desktop's own engine's. */
export const sharedPrWriteOrder = createPrWriteOrder();

export const {
  issue: issuePrWrite,
  accept: acceptPrWrite,
  stamp: stampPrWrite,
  fence: fencePrWrites,
  reset: resetPrWriteOrder,
} = sharedPrWriteOrder;
