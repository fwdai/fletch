/** Format a turn duration for the run timer. Under a minute → whole seconds
 *  (`38s`); a minute or more → minutes + zero-padded seconds (`3m 47s`). Never
 *  decimals, milliseconds, or an hh: field; negatives clamp to zero. One
 *  function drives both the live and the static (completed) variants. */
export function fmtDur(sec: number): string {
  const total = Math.max(0, Math.floor(sec));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m === 0 ? `${s}s` : `${m}m ${String(s).padStart(2, "0")}s`;
}
