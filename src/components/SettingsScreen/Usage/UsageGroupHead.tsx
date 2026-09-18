import type { ReactNode } from "react";

/** A section heading with room for a control on the right — the Cost / Tokens
 *  switch above the overview, the Model / Day switch above the breakdown. The
 *  control scopes to its section only; page-wide controls (the period picker)
 *  belong on the pane header instead. */
export function UsageGroupHead({ label, children }: { label: string; children?: ReactNode }) {
  return (
    <div className="usg-group-h flex-center">
      <div className="set-group-h mono text-xs">{label}</div>
      {children}
    </div>
  );
}
