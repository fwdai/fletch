import type { ReactNode } from "react";
import type { FeatureFlags } from "../../store";

/** Shared building blocks for the full-screen settings panes. These use the
 *  `.set-*` styling (distinct from the quick popover's `.sp-*`). */

/** A single feature-flag row: which flag it toggles plus its display copy.
 *  Shared by the panes that render lists of `SetToggle` rows. */
export interface FeatureItem {
  key: keyof FeatureFlags;
  title: string;
  sub: string;
}

export function SetHead({
  eyebrow,
  title,
  desc,
  actions,
}: {
  eyebrow: string;
  title: string;
  desc?: ReactNode;
  /** Optional controls aligned to the right of the title row. */
  actions?: ReactNode;
}) {
  return (
    <header className="set-head">
      <div className="set-head-top">
        <div className="set-head-titles">
          <div className="set-eyebrow mono">{eyebrow}</div>
          <h1 className="set-h1">{title}</h1>
        </div>
        {actions && <div className="set-head-actions">{actions}</div>}
      </div>
      {desc && <p className="set-lead">{desc}</p>}
    </header>
  );
}

export function SetGroup({
  label,
  last,
  children,
}: {
  label?: string;
  last?: boolean;
  children: ReactNode;
}) {
  return (
    <section className={`set-group ${last ? "last" : ""}`}>
      {label && <div className="set-group-h mono">{label}</div>}
      <div className="set-rows">{children}</div>
    </section>
  );
}

export function SetRow({
  title,
  sub,
  align,
  children,
}: {
  title: ReactNode;
  sub?: ReactNode;
  align?: "start";
  children?: ReactNode;
}) {
  return (
    <div className={`set-row ${align === "start" ? "align-start" : ""}`}>
      <div className="set-row-l">
        <div className="set-row-t">{title}</div>
        {sub && <div className="set-row-s">{sub}</div>}
      </div>
      {children && <div className="set-row-c">{children}</div>}
    </div>
  );
}

export function SetToggle({
  on,
  onClick,
}: {
  on: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="set-toggle"
      data-on={on ? "1" : "0"}
      role="switch"
      aria-checked={on}
      onClick={onClick}
    >
      <i />
    </button>
  );
}

export function SetSeg<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="set-seg">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={value === o.value ? "active" : ""}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
