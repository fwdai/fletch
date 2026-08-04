// Board/HoldControl.tsx — the two halves of a hold on a card: the chip that says
// it is held (and lets the user lift it), and the action that places one.
//
// Their own file rather than more branches inside ItemCard: the chip is the
// ghostbar's grammar (a band between the header and the body, ruled on without an
// expand), and placing a hold needs a *reason*, which means a small piece of local
// state no other part of the card has. The reason prompt reuses the strip's inline
// pattern — an input that appears in place, ⌘↵/Enter to commit, Escape to back
// out — so writing a hold reason feels like answering a question inline, not like
// opening a form.

import { type KeyboardEvent, useState } from "react";
import type { RoadmapEventActor } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";

/** Who stopped it, in the words the card uses elsewhere. The distinction matters:
 *  a hold the PM placed is news, one the user placed is a note to self. */
function heldBy(by: RoadmapEventActor | null): string {
  return by === "pm" ? "Held by the PM" : "Held";
}

/** The always-visible band on a held row: why nothing is happening, and the one
 *  gesture that changes it. Warn-toned like the queue note, because the row has
 *  stopped — but with an action, because unlike a queue note this one is only
 *  waiting on the person reading it. */
export function HoldChip({
  reason,
  by,
  onRelease,
}: {
  reason: string;
  by: RoadmapEventActor | null;
  /** Absent on a read-only board. */
  onRelease?: () => void;
}) {
  return (
    <div className="rm-hold flex-center text-xs">
      <Icon name="pause" size={11} />
      <span className="rm-hold-t">
        <strong>{heldBy(by)}</strong> — {reason}
      </span>
      <span className="grow" />
      {onRelease && (
        <Button variant="ghost" size="sm" onClick={onRelease}>
          <Icon name="play" size={11} /> Release
        </Button>
      )}
    </div>
  );
}

/** "Hold" as a secondary action, with the reason asked for in place.
 *
 *  The reason is required by the backend, so it is asked for here rather than
 *  sent blank and refused: a hold with no reason leaves the user a Release button
 *  and no idea what it undoes. Committing with an empty box simply does nothing,
 *  which is the same answer as Cancel and needs no error state to say so. */
export function HoldAction({ onHold }: { onHold: (reason: string) => void }) {
  const [reason, setReason] = useState<string | null>(null);

  if (reason == null) {
    return (
      <Button
        variant="ghost"
        size="sm"
        tip="Stop the queue from building this until you release it"
        onClick={() => setReason("")}
      >
        <Icon name="pause" size={11} /> Hold
      </Button>
    );
  }

  const commit = () => {
    const text = reason.trim();
    if (text) onHold(text);
    setReason(null);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") commit();
    if (e.key === "Escape") setReason(null);
  };

  return (
    <span className="rm-hold-ask flex-center">
      <TextInput
        // The input appears exactly where the button the user just pressed was,
        // so the caret belongs in it — otherwise the gesture is click, then click
        // again, to type the thing they opened the box to type.
        autoFocus
        className="rm-hold-input"
        placeholder="What has to be agreed first?"
        value={reason}
        onChange={(e) => setReason(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <Button variant="ghost" size="sm" onClick={() => setReason(null)}>
        Cancel
      </Button>
      <Button variant="primary" size="sm" onClick={commit}>
        <Icon name="pause" size={11} /> Hold
      </Button>
    </span>
  );
}
