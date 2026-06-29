import { Fragment } from "react";

export interface CrumbEntry {
  label: string;
  /** Render in mono — used for repo/agent name segments. */
  mono?: boolean;
  /** Highlight as the active segment. */
  active?: boolean;
  /** Color swatch (hue in OKLCH) shown left of this label. */
  swatchHue?: number;
}

export function Breadcrumb({ entries }: { entries: CrumbEntry[] }) {
  return (
    <div className="tb-crumb flex-center">
      {entries.map((c, i) => (
        <Fragment key={i}>
          {i > 0 && <span className="sep">/</span>}
          {c.swatchHue != null && (
            <span className="swatch" style={{ background: `oklch(0.5 0.08 ${c.swatchHue})` }} />
          )}
          <span className={(c.active ? "active " : "") + (c.mono ? "mono" : "")}>{c.label}</span>
        </Fragment>
      ))}
    </div>
  );
}
