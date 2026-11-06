// Board/ProjectHoldBanner.tsx — the whole board is stopped, said once.
//
// A banner rather than a chip per card: a project hold is a fact about the board,
// so repeating it on every row would be the same sentence five times, and the row
// it is least true of is whichever one the user is reading. Same grammar as the
// board's other bands (a flat strip between the header and the scroller, one
// accent line, an action on the right) — warn-toned, because unlike a proposal
// this is something already stopped.
//
// What it deliberately does not do is disable the cards' Queue buttons. Queueing
// under a hold is a legitimate act: the user is saying "build this when we
// resume", and the queue keeps its order in the meantime. The banner is what
// explains why nothing has started.

import type { RoadmapProjectHold } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { formatAge } from "@/util/format";

export function ProjectHoldBanner({
  hold,
  onRelease,
}: {
  hold: RoadmapProjectHold;
  /** Absent on a read-only board. Releasing is the user's alone — the PM can
   *  place this hold and has no way to lift it. */
  onRelease?: () => void;
}) {
  return (
    <div className="rm-phold flex-center text-xs">
      <Icon name="pause" size={11} className="rm-phold-i" />
      <span className="rm-phold-t">
        <strong>{hold.held_by === "pm" ? "The PM held this board" : "You held this board"}</strong>{" "}
        — {hold.reason}
      </span>
      <span className="rm-phold-age mono">{formatAge(hold.created_at, Date.now())}</span>
      <span className="grow" />
      {onRelease && (
        <Button variant="primary" size="sm" onClick={onRelease}>
          <Icon name="play" size={11} /> Release
        </Button>
      )}
    </div>
  );
}
