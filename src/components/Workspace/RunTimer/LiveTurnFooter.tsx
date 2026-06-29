import type { TurnTiming } from "../../../adapters/types";
import { Icon } from "../../Icon";
import { LiveTimer } from "./LiveTimer";

/** The running-turn footer: activity dots · status label · clock · live timer.
 *  Part of the open turn (hangs off the agent's accent left-rule via `.writing`),
 *  not a turn boundary — so it omits the seam the static footer draws. The dots
 *  drop when paused; the timer freezes (see LiveTimer). */
export function LiveTurnFooter({ label, timing }: { label: string; timing: TurnTiming }) {
  const paused = timing.runningSince == null;
  return (
    <div className="writing flex-center">
      {!paused && (
        <span className="dots">
          <i />
          <i />
          <i />
        </span>
      )}
      <span>{label}</span>
      <span className="turn-sep">·</span>
      <Icon name="clock" size={11} className="turn-clock-i" />
      <LiveTimer timing={timing} />
    </div>
  );
}
