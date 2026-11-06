import { type ReactNode, useState } from "react";
import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import type { FeatureFlags } from "@/storage/preferences";

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
  eyebrowAside,
  title,
  desc,
  actions,
}: {
  eyebrow: string;
  /** Optional controls aligned to the right of the eyebrow row — e.g. the
   *  Customize section switch, which sits above the (tab-dependent) title. */
  eyebrowAside?: ReactNode;
  title: string;
  /** One line naming the category of settings on the page. How things work
   *  belongs on the rows. */
  desc?: ReactNode;
  /** Optional controls aligned to the right of the title row. */
  actions?: ReactNode;
}) {
  return (
    <header className="set-head">
      <div className="set-head-eyebrow">
        <div className="set-eyebrow mono text-xs">{eyebrow}</div>
        {eyebrowAside && <div className="set-head-eyebrow-aside flex-center">{eyebrowAside}</div>}
      </div>
      <div className="set-head-top">
        <div className="set-head-titles">
          <h1 className="set-h1 text-3xl">{title}</h1>
        </div>
        {actions && <div className="set-head-actions flex-center">{actions}</div>}
      </div>
      {desc && <p className="set-lead text-base">{desc}</p>}
    </header>
  );
}

export function SetGroup({
  label,
  aside,
  sub,
  last,
  children,
}: {
  label?: string;
  /** Sits at the right end of the label line: a status, not a control. */
  aside?: ReactNode;
  /** One line under the label about the whole group. */
  sub?: ReactNode;
  last?: boolean;
  children: ReactNode;
}) {
  const head = label && <div className="set-group-h mono text-xs">{label}</div>;
  return (
    <section className={`set-group ${last ? "last" : ""}`}>
      {aside ? (
        <div className="set-group-top flex-center">
          {head}
          {aside}
        </div>
      ) : (
        head
      )}
      {sub && <div className="set-row-s text-sm">{sub}</div>}
      <div className="set-rows">{children}</div>
    </section>
  );
}

export function SetRow({
  icon,
  title,
  sub,
  align,
  compact,
  inherited,
  children,
}: {
  /** Sits beside the title and description, spanning both. */
  icon?: ReactNode;
  /** One tight line, title and description side by side: for a list of things
   *  rather than a setting to read. */
  compact?: boolean;
  title: ReactNode;
  /** Two lines at most. Leave it out when the title and control say enough. */
  sub?: ReactNode;
  align?: "start";
  /** A row over an app default: while it overrides it, the run-config rows'
   *  dot and a control that puts the default back sit before the control. */
  inherited?: { overridden: boolean; onRevert: () => void };
  children?: ReactNode;
}) {
  const label = (
    <div className="set-row-l">
      <div className="set-row-t text-base">{title}</div>
      {sub && <div className="set-row-s text-sm">{sub}</div>}
    </div>
  );
  return (
    <div
      className={`set-row flex-center ${align === "start" ? "align-start" : ""} ${
        compact ? "compact" : ""
      }`}
    >
      {icon ? (
        <div className="set-row-lead flex-center">
          {icon}
          {label}
        </div>
      ) : (
        label
      )}
      {children && (
        <div className="set-row-c flex-center">
          {inherited?.overridden && (
            <>
              <span className="rs-source iflex-center">
                <span className="dot" />
              </span>
              <IconButton size="sm" tip="Revert to the app default" onClick={inherited.onRevert}>
                <Icon name="refresh" size={12} />
              </IconButton>
            </>
          )}
          {children}
        </div>
      )}
    </div>
  );
}

/** Rows most people never need, behind a quiet link at the end of a group:
 *  the escape hatches for someone who knows what they are changing. Closed on
 *  every visit. */
export function SetDisclosure({ label, children }: { label: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="set-disclosure">
      <button
        type="button"
        className="set-disclosure-btn iflex-center text-sm"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {label}
        <Icon name="chevD" size={12} className={open ? "open" : undefined} />
      </button>
      {open && <div className="set-rows">{children}</div>}
    </div>
  );
}

export function SetToggle({
  on,
  onClick,
  disabled,
  tip,
}: {
  on: boolean;
  onClick: () => void;
  disabled?: boolean;
  /** Why the toggle can't be flipped. Only shown while `disabled` — that's the
   *  state that needs explaining. */
  tip?: string;
}) {
  const toggle = (
    <button
      type="button"
      className="set-toggle"
      data-on={on ? "1" : "0"}
      role="switch"
      aria-checked={on}
      disabled={disabled}
      style={disabled ? { opacity: 0.4, cursor: "not-allowed" } : undefined}
      onClick={onClick}
    >
      <i />
    </button>
  );
  // A disabled button receives no pointer events in the WebView, so the
  // tooltip trigger has to be a wrapper or it would never show.
  return disabled && tip ? (
    <span className="tip set-toggle-tip" data-tip={tip}>
      {toggle}
    </span>
  ) : (
    toggle
  );
}

export function SetSeg<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  /** `tip` is optional hover detail for an option whose short label can't say
   *  everything — e.g. exactly what window "Past 24h" means. */
  options: { value: T; label: string; tip?: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="set-seg">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={`${value === o.value ? "active" : ""}${o.tip ? " tip" : ""}`}
          data-tip={o.tip}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
