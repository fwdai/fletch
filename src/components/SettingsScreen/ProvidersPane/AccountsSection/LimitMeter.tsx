import type { LimitWindow } from "@/api/types/providers";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { limitTone, resetLabel, resetShort } from "./limitsFormat";

/** One plan window of an account: its name, how much is used, and when it
 *  starts over. A window the source didn't report shows a dash rather than a
 *  0% that would claim the window is untouched. `note` is appended to the
 *  reset line — a model window older than the reading says so here, on the
 *  meter it is about.
 *
 *  `compact` puts the whole meter on one line for a list read at a glance:
 *  name, bar, share and just the time left, with the reset line in that
 *  label's tooltip. There a window that has reset since the reading is dimmed, since
 *  no line says its number is no longer true. */
export function LimitMeter({
  label,
  window,
  nowMs,
  note,
  compact,
}: {
  label: string;
  window: LimitWindow | null;
  nowMs: number;
  note?: string | null;
  compact?: boolean;
}) {
  const percent = window ? Math.round(window.percent) : null;
  const tone = limitTone(percent);
  const reset = window ? resetLabel(window.resets_at, nowMs) : null;
  const foot = [reset, note].filter(Boolean).join(" · ");
  const name = <span className="set-prov-limit-name">{label}</span>;
  const shareText = percent === null ? "—" : `${percent}%`;
  const share = <span className="set-prov-limit-pct mono">{shareText}</span>;
  const bar = <ProgressBar percent={percent ?? 0} className="set-prov-limit-bar" />;

  if (compact) {
    const left = window ? resetShort(window.resets_at, nowMs) : null;
    const passed = window?.resets_at != null && window.resets_at * 1000 <= nowMs ? "passed" : "";
    // The reset line hangs off the time left it spells out; a window with no
    // reset time carries its note on the share instead.
    const tipOn = (className: string, on: boolean) =>
      on && foot ? { className: `${className} tip`, "data-tip": foot } : { className };
    return (
      <div className={`set-prov-limit compact text-xs ${tone} ${passed}`}>
        {name}
        {bar}
        <span {...tipOn("set-prov-limit-pct mono", !left)}>{shareText}</span>
        <span {...tipOn("set-prov-limit-left", !!left)}>{left}</span>
      </div>
    );
  }

  return (
    <div className={`set-prov-limit ${tone}`}>
      <div className="set-prov-limit-top flex-center text-xs">
        {name}
        {share}
      </div>
      {bar}
      {foot && <div className="set-prov-limit-reset text-xs">{foot}</div>}
    </div>
  );
}
