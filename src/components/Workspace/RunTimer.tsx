// Per-turn run timing in the chat feed. One formatter drives both variants:
// a live timer that ticks on the open turn, and a static "Ran …" record under
// every completed turn. See the Run Timer component spec.

import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { CopyButton } from "@/components/ui/CopyButton";

/** Whole seconds under a minute; `{m}m {ss}s` (zero-padded seconds) at a
 *  minute or more; `{h}h {mm}m {ss}s` at an hour or more. Never a leading
 *  `0h`/`0m`, never decimals. */
export function fmtDur(sec: number): string {
  sec = Math.max(0, Math.floor(sec));
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  if (h > 0) {
    return `${h}h ${String(m).padStart(2, "0")}m ${String(s).padStart(2, "0")}s`;
  }
  return m === 0 ? `${s}s` : `${m}m ${String(s).padStart(2, "0")}s`;
}

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

/** The footer closing an ended turn: the quiet "Ran 38s" record on the left and
 *  a dimmed copy of the turn's response on the right. Doubles as the seam
 *  between turns (the hairline lives in CSS). */
export function TurnFooter({ runSec, copyText }: { runSec: number; copyText: string }) {
  return (
    <div className="turn-meta">
      <Icon name="clock" size={11} />
      <span>Ran {fmtDur(runSec)}</span>
      {copyText && <CopyButton text={copyText} className="turn-copy" />}
    </div>
  );
}
