import { useState } from "react";
import type { RoadmapItem, RoadmapOrderProposal } from "@/api";
import { Icon } from "@/components/Icon";
import { BoardRulingActions } from "./DecisionBar";

/** One line of the proposed sequence: where the item would sit, what it is, and
 *  whether that is a change. */
export interface OrderPreviewRow {
  code: string;
  title: string;
  /** This item's position in the sequence differs from where it sits now — the
   *  only rows worth marking, since an accepted order rewrites every rank. */
  moved: boolean;
}

/** Pair the proposed sequence with the board it would replace. Pure, and quietly
 *  tolerant: a code the board no longer holds still renders (the ruling will
 *  refuse the stale ask and say so — the preview's job is to show what was
 *  asked, not to pre-empt the verdict). */
export function orderPreview(codes: string[], orderable: RoadmapItem[]): OrderPreviewRow[] {
  const now = new Map(orderable.map((i, at) => [i.code, { title: i.title, at }]));
  return codes.map((code, at) => {
    const current = now.get(code);
    return {
      code,
      title: current?.title ?? "(no longer on the board)",
      moved: current != null && current.at !== at,
    };
  });
}

/** The PM's pending whole-board reordering, ruled on above the groups.
 *
 *  The ghost batch bar's grammar, one altitude up: an always-visible accent bar
 *  saying what was proposed and why, with Accept and Decline. What it adds is the
 *  sequence itself — a numbered list, because the ask *is* an order and no
 *  summary of it is honest. Rows whose position would change are marked; the rest
 *  are context. Deliberately no drag preview or animation: the user is reading a
 *  list, not watching one. */
export function OrderProposalBar({
  proposal,
  orderable,
  onAccept,
  onDecline,
}: {
  proposal: RoadmapOrderProposal;
  /** The board's orderable rows, in current board order. */
  orderable: RoadmapItem[];
  /** Absent on a read-only board. */
  onAccept?: () => void;
  onDecline?: () => void;
}) {
  const [open, setOpen] = useState(true);
  const rows = orderPreview(proposal.codes, orderable);
  const changed = rows.filter((r) => r.moved).length;

  return (
    <div className="rm-order">
      <div className="rm-order-h flex-center text-xs">
        <button
          type="button"
          className="rm-order-t flex-center"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
        >
          <Icon name="sparkle" size={11} />
          <span className="rm-order-l truncate">
            <strong>PM proposes a new order</strong>
            {proposal.note ? ` — ${proposal.note}` : ""}
          </span>
          <span className="rm-order-n mono">
            {changed} of {rows.length} move
          </span>
          <Icon name="chevD" size={9} className="rm-order-chev" />
        </button>
        <span className="grow" />
        {/* The ruling trio lives in DecisionBar.tsx, with the batch bar's — this
            bar's own contribution is the sequence, not the buttons. */}
        <BoardRulingActions onAccept={onAccept} onDecline={onDecline} />
      </div>
      {open && (
        <ol className="rm-order-list text-xs">
          {rows.map((r) => (
            <li key={r.code} className={`flex-center ${r.moved ? "moved" : ""}`}>
              <span className="rm-order-i mono">{r.code}</span>
              <span className="rm-order-title truncate">{r.title}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
