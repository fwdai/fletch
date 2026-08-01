import type { ReactNode } from "react";

/** One horizon section of the board — a labelled header with a right-aligned
 *  count over a rule-anchored stack of rows. */
export function HorizonGroup({
  label,
  note,
  count,
  empty,
  children,
}: {
  label: string;
  note: string;
  /** Committed rows only — proposed ones aren't real yet, so they don't count
   *  here either (this is the same number the page header shows). */
  count: number;
  /** Nothing to render at all, committed or proposed. */
  empty: boolean;
  children: ReactNode;
}) {
  return (
    <section className="rm-group">
      <div className="rm-group-h">
        <span className="rm-group-l mono text-xs">{label}</span>
        <span className="rm-group-n text-xs">{note}</span>
        <span className="rm-group-c mono text-xs">{count}</span>
      </div>
      <div className="rm-group-body">
        {empty ? <div className="rm-empty text-xs">Nothing here yet.</div> : children}
      </div>
    </section>
  );
}
