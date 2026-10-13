import type { LimitWindow } from "@desktop/api/types/providers";
import { resetLabel } from "@desktop/components/SettingsScreen/ProvidersPane/AccountsSection/limitsFormat";

/** Past this share of a window the meter warns; at the top it reads as spent.
 *  The desktop's thresholds. */
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
        ? " full"
        : percent >= WARN_PERCENT
          ? " warn"
          : "";
  const reset = window ? resetLabel(window.resets_at, nowMs) : null;
  return (
    <div className={`pv-limit${tone}`}>
      <div className="pv-limit-top">
        <span>{label}</span>
        <span className="pv-limit-pct">{percent === null ? "—" : `${percent}%`}</span>
      </div>
      <div className="pv-bar">
        <div className="pv-bar-fill" style={{ width: `${Math.min(100, percent ?? 0)}%` }} />
      </div>
      {reset && <div className="pv-limit-reset">{reset}</div>}
    </div>
  );
}
