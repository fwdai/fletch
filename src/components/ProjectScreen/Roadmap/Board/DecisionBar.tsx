import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";

/** The always-visible bar a card carries while something on it is waiting to be
 *  ruled on: a PM suggestion the user hasn't admitted (a ghost row), or a PM
 *  delta against a row that is already on the roadmap.
 *
 *  One component for both because they are one gesture — read the ask, accept or
 *  decline — and only the words and the handlers differ. It sits outside the
 *  card's collapsible body on purpose: ruling on a proposal must never cost an
 *  expand (reading it first is what the expand is for).
 *
 *  Either action may be absent (a read-only board hands neither), in which case
 *  the caller shouldn't render the bar at all — a decision surface with no
 *  decision on it is furniture. */
export function DecisionBar({
  label,
  note,
  variant = "ghost",
  acceptLabel = "Accept",
  queueLabel,
  declineLabel,
  onAccept,
  onAcceptQueue,
  onDecline,
}: {
  /** What is being asked, in a few words ("Proposed — not on the roadmap yet").
   *  Always the headline; how loudly it is drawn is the variant's business. */
  label: string;
  /** The PM's one-line rationale, when the ask carries one. */
  note?: string | null;
  /** `ghost` for a row that isn't on the roadmap yet (transparent, dashed into
   *  the card above it); `prop` for a delta on a real row (tinted, so the
   *  decision surface stands out from a body that *is* the roadmap). */
  variant?: "ghost" | "prop";
  acceptLabel?: string;
  /** Label for the second accept, when the caller has one — see `onAcceptQueue`. */
  queueLabel?: string | null;
  declineLabel: string;
  onAccept?: () => void;
  /** Accept *and* queue, in one click (a ghost row's `queue: true` accept). Only
   *  offered while the project's autoqueue dial is off: with it on, `onAccept`
   *  already queues and this would be the same button twice. Drawn as an outline
   *  so the plain accept stays the primary — the extra autonomy is the opt-in. */
  onAcceptQueue?: () => void;
  onDecline?: () => void;
}) {
  return (
    <div className={`rm-decbar flex-center ${variant}`}>
      <span className="rm-decbar-l text-xs truncate">
        <strong>{label}</strong>
        {note ? ` — ${note}` : ""}
      </span>
      <span className="grow" />
      {onDecline && (
        <Button variant="ghost" size="sm" onClick={onDecline}>
          {declineLabel}
        </Button>
      )}
      {onAcceptQueue && queueLabel && (
        <Button variant="outline" size="sm" onClick={onAcceptQueue}>
          <Icon name="zap" size={11} /> {queueLabel}
        </Button>
      )}
      {onAccept && (
        <Button variant="primary" size="sm" onClick={onAccept}>
          <Icon name="check" size={11} /> {acceptLabel}
        </Button>
      )}
    </div>
  );
}

/** The *board*-scoped ruling's buttons: the batch of proposals above the horizon
 *  groups, and the PM's whole-board order ask.
 *
 *  Actions only, deliberately — where a card's bar is one fixed row (headline,
 *  rationale, buttons), a board bar's left half differs per caller: the batch bar
 *  carries a count and a hint, the order bar carries a collapsible numbered
 *  sequence. What they share is the trio on the right, which both used to
 *  hand-roll — three near-identical copies of markup [`DecisionBar`] exists to
 *  own, drifting one button at a time. This is that markup's one home for them.
 *
 *  Drawn with the board bars' own classes (`rm-props-*`) rather than the card
 *  bar's `Button`s: a board-level bar is an accent band across the column, and the
 *  two vocabularies are a deliberate visual distinction between "this row" and
 *  "this board". Any action may be absent — a read-only board hands none, and a
 *  caller with nothing to offer shouldn't render a bar at all. */
export function BoardRulingActions({
  acceptLabel = "Accept",
  queueLabel,
  declineLabel = "Decline",
  onAccept,
  onAcceptQueue,
  onDecline,
}: {
  acceptLabel?: string;
  /** Label for the second accept, when the caller has one — see
   *  [`DecisionBar`]'s `onAcceptQueue`. */
  queueLabel?: string | null;
  declineLabel?: string;
  onAccept?: () => void;
  onAcceptQueue?: () => void;
  onDecline?: () => void;
}) {
  return (
    <>
      {onDecline && (
        <button type="button" className="rm-props-x" onClick={onDecline}>
          {declineLabel}
        </button>
      )}
      {onAcceptQueue && queueLabel && (
        <button type="button" className="rm-props-q iflex-center" onClick={onAcceptQueue}>
          <Icon name="zap" size={11} /> {queueLabel}
        </button>
      )}
      {onAccept && (
        <button type="button" className="rm-props-ok iflex-center" onClick={onAccept}>
          <Icon name="check" size={11} /> {acceptLabel}
        </button>
      )}
    </>
  );
}
