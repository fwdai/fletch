/** Thin determinate/indeterminate progress bar for a download or other
 *  long-running transfer. `percent` is 0–100; `null` means the total isn't
 *  known (no Content-Length, or no event yet), so the bar sweeps instead of
 *  showing a bogus 0% — see `downloadPercent` in `@/util/format`. */
export function ProgressBar({
  percent,
  className,
}: {
  percent: number | null;
  className?: string;
}) {
  const classes = ["ui-progress", percent === null && "indet", className].filter(Boolean).join(" ");
  return (
    <div className={classes}>
      <i style={percent === null ? undefined : { width: `${percent}%` }} />
    </div>
  );
}
