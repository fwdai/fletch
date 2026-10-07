import type { LimitWindow } from "@/api/types/providers";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { resetLabel } from "./limitsFormat";

/** Past this share of a window the meter warns; at the top it reads as spent. */
const WARN_PERCENT = 75;
const FULL_PERCENT = 95;

/** One plan window of an account: its name, how much is used, and when it
 *  starts over. A window the source didn't report shows a dash rather than a
 *  0% that would claim the window is untouched. */
export function LimitMeter({
  label,
  window,
  nowMs,
}: {
  label: string;
  window: LimitWindow | null;
  nowMs: number;
}) {
  const percent = window ? Math.round(window.percent) : null;
  const tone =
    percent === null
      ? ""
      : percent >= FULL_PERCENT
        ? "full"
        : percent >= WARN_PERCENT
          ? "warn"
          : "";
  const reset = window ? resetLabel(window.resets_at, nowMs) : null;
  return (
    <div className={`set-prov-limit ${tone}`}>
      <div className="set-prov-limit-top flex-center text-xs">
        <span className="set-prov-limit-name">{label}</span>
        <span className="set-prov-limit-pct mono">{percent === null ? "—" : `${percent}%`}</span>
      </div>
      <ProgressBar percent={percent ?? 0} className="set-prov-limit-bar" />
      {reset && <div className="set-prov-limit-reset text-xs">{reset}</div>}
    </div>
  );
}
