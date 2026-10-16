import type { LimitWindow } from "@/api/types/providers";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { resetLabel } from "./limitsFormat";

/** Past this share of a window the meter warns; at the top it reads as spent. */
const WARN_PERCENT = 75;
const FULL_PERCENT = 95;

/** One plan window of an account: its name, how much is used, and when it
 *  starts over. A window the source didn't report shows a dash rather than a
 *  0% that would claim the window is untouched. `note` is appended to the
 *  reset line — a model window older than the reading says so here, on the
 *  meter it is about. */
export function LimitMeter({
  label,
  window,
  nowMs,
  note,
}: {
  label: string;
  window: LimitWindow | null;
  nowMs: number;
  note?: string | null;
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
  const foot = [reset, note].filter(Boolean).join(" · ");
  return (
    <div className={`set-prov-limit ${tone}`}>
      <div className="set-prov-limit-top flex-center text-xs">
        <span className="set-prov-limit-name">{label}</span>
        <span className="set-prov-limit-pct mono">{percent === null ? "—" : `${percent}%`}</span>
      </div>
      <ProgressBar percent={percent ?? 0} className="set-prov-limit-bar" />
      {foot && <div className="set-prov-limit-reset text-xs">{foot}</div>}
    </div>
  );
}
