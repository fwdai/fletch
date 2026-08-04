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
  declineLabel,
  onAccept,
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
  declineLabel: string;
  onAccept?: () => void;
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
      {onAccept && (
        <Button variant="primary" size="sm" onClick={onAccept}>
          <Icon name="check" size={11} /> {acceptLabel}
        </Button>
      )}
    </div>
  );
}
