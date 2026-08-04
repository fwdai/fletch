// What the user is being asked to rule on, per card and per board — the one
// answer the decision bar, the diff and the batch count all read.
//
// Two things can be pending against a single row at once: the row itself may be a
// ghost the PM proposed and the user hasn't admitted, and there may be a *further*
// PM ask against it (a retitle, a re-scoped `accept`, a discard). The board used to
// disagree with itself about that combination in three places at the same time:
// the card suppressed the ask's decision bar on a ghost (rightly — two Accepts
// meaning different things is a coin flip) but rendered the ask's *diff*
// regardless, and the batch bar dropped the ask from its count. The backend, for
// its part, kept quoting the ask to the PM as pending, because it is: `is_rulable`
// includes `proposed`. So the user saw a diff of a change with no way to rule it,
// and a count that said there was nothing there.
//
// The shape that fixes it is one ruling per row. A ghost carrying an ask is not two
// questions, it is one: "do you want the revised item?" — and accepting it admits
// the row and then applies the patch, in that order (the row is `open` by then,
// which is still rulable, so the patch lands). Declining it discards the ghost, and
// the ask dies with the row (`roadmap_delete_item` reads the pending proposal, the
// FK cascades it away, and the deletion is broadcast on both streams).
//
// Pure and tested so the three surfaces cannot drift apart again.

import type { RoadmapProposal } from "@/api";

/** Which ruling a card carries.
 *
 *  - `none` — nothing pending; the card is just a row.
 *  - `ghost` — a proposed row awaiting admission.
 *  - `revised` — a proposed row *and* a PM ask against it: one ruling.
 *  - `ask` — a PM ask against a row that is already on the roadmap. */
export type RulingKind = "none" | "ghost" | "revised" | "ask";

export interface CardRuling {
  kind: RulingKind;
  /** The pending ask this ruling covers, or null. */
  proposal: RoadmapProposal | null;
  /** The bar's headline. Empty for `none`, which draws no bar. */
  label: string;
  /** What the decline button says. */
  declineLabel: string;
  /** How loudly the bar is drawn: `ghost` for a row that isn't on the roadmap
   *  yet, `prop` for a delta against one that is. A revised ghost is still a
   *  ghost — the row's admission is the bigger half of the question. */
  variant: "ghost" | "prop";
  /** Does accepting put the row on the roadmap (`proposed → open`)? */
  admits: boolean;
  /** Does accepting apply the pending patch? */
  appliesPatch: boolean;
  /** Does declining delete the row, rather than leaving it as it is? */
  declineRemovesRow: boolean;
  /** Should the expanded body show the pending patch's field-by-field diff? True
   *  exactly when the bar above it can rule that patch — a diff the user cannot
   *  act on is the bug this flag exists to make impossible. */
  showsDiff: boolean;
}

const NONE: CardRuling = {
  kind: "none",
  proposal: null,
  label: "",
  declineLabel: "",
  variant: "prop",
  admits: false,
  appliesPatch: false,
  declineRemovesRow: false,
  showsDiff: false,
};

/** The ruling for one card.
 *
 *  `ghost` is the row's own status question and `proposal` is the PM's ask against
 *  it; every combination of the two has exactly one answer here. */
export function cardRuling(ghost: boolean, proposal: RoadmapProposal | null = null): CardRuling {
  if (ghost && proposal) {
    return {
      kind: "revised",
      proposal,
      // Named for what the user gets, not for the two writes it takes: the item
      // as the PM would now have it. A `discard` ask on a ghost is the PM
      // withdrawing its own suggestion, which the single Discard already does —
      // so the accept still admits, and the patch (there is none) is a no-op.
      label:
        proposal.kind === "discard"
          ? "Proposed, and the PM has since withdrawn it"
          : "Proposed, with a revision — accept both together",
      declineLabel: "Discard",
      variant: "ghost",
      admits: true,
      appliesPatch: proposal.kind === "update",
      declineRemovesRow: true,
      showsDiff: proposal.kind === "update",
    };
  }
  if (ghost) {
    return {
      kind: "ghost",
      proposal: null,
      label: "Proposed — not on the roadmap yet",
      declineLabel: "Discard",
      variant: "ghost",
      admits: true,
      appliesPatch: false,
      declineRemovesRow: true,
      showsDiff: false,
    };
  }
  if (proposal) {
    return {
      kind: "ask",
      proposal,
      label: proposal.kind === "discard" ? "PM proposes discarding" : "PM proposes changes",
      declineLabel: "Decline",
      variant: "prop",
      admits: false,
      appliesPatch: true,
      declineRemovesRow: false,
      showsDiff: proposal.kind === "update",
    };
  }
  return NONE;
}

// ─────────────────────────── the board's pending set ───────────────────────────

/** Everything the user owes this board a ruling on, by kind.
 *
 *  Four kinds, not two. The batch bar counted ghosts and item asks and silently
 *  ignored the board-scoped pair — the PM's whole-board order ask and its brief
 *  update — both of which are pending deltas the user must rule, and both of which
 *  are rendered somewhere else on the same screen. Counting two of four made the
 *  board's one summary number wrong. */
export interface PendingDeltas {
  /** Every pending delta, board-scoped ones included. */
  total: number;
  /** Proposed rows awaiting admission. */
  ghosts: number;
  /** Pending per-item PM asks — ghost-targeted ones included, because a revised
   *  ghost is one ruling that covers both. */
  asks: number;
  /** The whole-board order ask: 0 or 1. */
  order: number;
  /** The brief update: 0 or 1. */
  brief: number;
  /** The subset the batch bar itself can rule in one click: the card-scoped
   *  deltas. The other two have their own surfaces (the order bar below, the
   *  Product brief tab), so its buttons must never claim them. */
  batch: number;
  /** Ask ids to accept, after the ghosts are admitted — order matters: a patch
   *  against a row nobody has accepted is refused by the same gate that lets one
   *  through a moment later (`is_rulable`). */
  askIds: readonly string[];
  /** Ask ids that can be *declined* on their own. A ghost's ask is not one of
   *  them: discarding the ghost deletes the row and cascades the ask away, so
   *  declining it separately would be a write against a row that is about to not
   *  exist. */
  declinableAskIds: readonly string[];
}

export function pendingDeltas(input: {
  /** Ids of the board's proposed rows. */
  ghostIds: readonly string[];
  /** Every pending per-item ask on the board. */
  asks: readonly Pick<RoadmapProposal, "id" | "item_id">[];
  orderProposal: unknown | null;
  briefProposal: unknown | null;
}): PendingDeltas {
  const ghostIds = new Set(input.ghostIds);
  const ghosts = ghostIds.size;
  const asks = input.asks.length;
  const order = input.orderProposal ? 1 : 0;
  const brief = input.briefProposal ? 1 : 0;
  return {
    total: ghosts + asks + order + brief,
    ghosts,
    asks,
    order,
    brief,
    batch: ghosts + asks,
    askIds: input.asks.map((a) => a.id),
    declinableAskIds: input.asks.filter((a) => !ghostIds.has(a.item_id)).map((a) => a.id),
  };
}

/** The batch bar's one line, naming what is pending — including the two kinds its
 *  own buttons don't rule, so the number above it and the screen around it agree.
 *  Empty when nothing is pending elsewhere. */
export function pendingElsewhere(d: PendingDeltas): string {
  const parts: string[] = [];
  if (d.order) parts.push("a new order");
  if (d.brief) parts.push("a brief update");
  return parts.length ? `Also pending: ${parts.join(" and ")}.` : "";
}
