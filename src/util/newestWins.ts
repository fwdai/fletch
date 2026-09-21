// "Newest wins": the ordering rule for a read that several code paths refetch
// and then apply wholesale.
//
// Two of them can be in flight at once — a switch and a handshake, two deletes
// in a row, a reconnect while the user is already reloading — and their
// responses can resolve out of order. A blind write then lets the OLDER
// response, taken before something the newer one reflects, land last and
// overwrite it. That is what made a just-deleted agent flash back into the
// sidebar (store/refreshWorkspace), and what let an older `approvals_list`
// replace a queue the newer one had already answered for.
//
// The rule is one monotonic token shared by every site that refetches the same
// thing: each call stamps the next generation, and only the newest generation's
// response is applied. Older, slower responses are dropped. Anything a claim
// accumulates while it is in flight (an event buffer, say) belongs to that
// claim for the same reason — it is only ever folded into the snapshot the
// claim will write, and a superseded claim writes nothing.
//
// What this does NOT catch is the other ordering: the newest fetch's response
// was produced BEFORE something the user has since done, so it does not reflect
// it yet. That needs its own answer at each site (see `applyPendingHides`, and
// `replayApprovalEvents` in ./publishApprovals).

/** One refetch's place in the order. */
export interface Claim {
  /** True while no later claim has been taken — i.e. this response is still the
   *  newest one, and is the one to apply. */
  current(): boolean;
}

/** One order, for one thing that is refetched. Hold it at module scope beside
 *  the action that refetches, so every call site shares the same sequence. */
export function newestWins(): { claim: () => Claim } {
  let generation = 0;
  return {
    claim() {
      const mine = ++generation;
      return { current: () => mine === generation };
    },
  };
}
