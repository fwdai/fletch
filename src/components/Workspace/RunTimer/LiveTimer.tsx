import { useEffect, useState } from "react";
import type { TurnTiming } from "../../../adapters/types";
import { fmtDur } from "./fmtDur";

/** Live elapsed in ms for an open turn: accumulated active time plus the open
 *  span (if any). Recomputed from the stored timestamp — never an incrementing
 *  counter — so a backgrounded tab can't drift. */
export function liveElapsedMs(timing: TurnTiming, now: number): number {
  return timing.activeMs + (timing.runningSince != null ? now - timing.runningSince : 0);
}

/** The ticking elapsed value for the open turn (mono, accent). Ticks once a
 *  second while running; freezes at the accumulated value when paused
 *  (`runningSince == null`), where it conveys the awaiting-input state. */
export function LiveTimer({ timing }: { timing: TurnTiming }) {
  const running = timing.runningSince != null;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [running]);

  return <span className="turn-clock">{fmtDur(liveElapsedMs(timing, now) / 1000)}</span>;
}
