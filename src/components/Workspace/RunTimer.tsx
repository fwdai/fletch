// Per-turn run timing in the chat feed. One formatter drives both variants:
// a live timer that ticks on the open turn, and a static "Ran …" record under
// every completed turn. See the Run Timer component spec.

import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { CopyButton } from "@/components/ui/CopyButton";
import { fmtDur } from "@/util/format";
import { ForkMenu } from "./ForkMenu";

// `fmtDur` lives in util/format so the phone's turn footer can say "Ran 4m 12s"
// without this file's desktop chrome.
export { fmtDur };

/** Live elapsed value, recomputed from a fixed start timestamp every second so
 *  it never drifts when the tab is backgrounded. `startedAt` is epoch millis. */
export function LiveTimer({ startedAt }: { startedAt: number }) {
  const [sec, setSec] = useState(() => Math.floor((Date.now() - startedAt) / 1000));
  useEffect(() => {
    const tick = () => setSec(Math.floor((Date.now() - startedAt) / 1000));
    tick(); // re-seed immediately when the open turn changes (new start)
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, [startedAt]);

  return <span className="turn-clock">{fmtDur(sec)}</span>;
}

/** The footer closing an ended turn: the quiet "Ran 38s" record on the left and,
 *  on the right, a dimmed copy of the turn's response plus a "fork from here"
 *  action. Doubles as the seam between turns (the hairline lives in CSS).
 *
 *  `agentId` + `forkTurnId` (the last turn above this footer) are what the fork
 *  action anchors the new conversation on; no turn id, no fork action. */
export function TurnFooter({
  runSec,
  copyText,
  agentId,
  forkTurnId,
}: {
  runSec: number;
  copyText: string;
  agentId: string;
  forkTurnId: string | undefined;
}) {
  return (
    <div className="turn-meta">
      <Icon name="clock" size={11} />
      <span>Ran {fmtDur(runSec)}</span>
      {copyText && <CopyButton text={copyText} className="turn-copy" />}
      {forkTurnId && (
        <ForkMenu agentId={agentId} turnId={forkTurnId} tip="Fork from here" compact />
      )}
    </div>
  );
}
