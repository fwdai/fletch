import { Icon } from "../../Icon";
import { fmtDur } from "./fmtDur";

/** The static record of a completed turn — quiet, muted, no motion. Its
 *  border-top (drawn by `.turn-meta`) doubles as the seam between turns. */
export function TurnFooter({ runSec }: { runSec: number }) {
  return (
    <div className="turn-meta">
      <Icon name="clock" size={11} />
      <span>Ran {fmtDur(runSec)}</span>
    </div>
  );
}
