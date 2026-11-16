import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { Icon } from "@/components/Icon";

interface Props extends Omit<ComponentPropsWithoutRef<"button">, "disabled"> {
  /** The row's leading glyph, at the card's 26px icon size. */
  icon: ReactNode;
  label: string;
  /** Muted mono text at the row's far end: a version, a count, the pick in force. */
  note?: ReactNode;
  /** The row is the current selection. */
  active?: boolean;
  /** The row's side panel is the one showing. */
  hot?: boolean;
  /** Dimmed and inert. The caller's handlers still fire (the row stays
   *  hover-capable, see below), so they guard on this themselves. */
  disabled?: boolean;
  /** Why the row is disabled, shown on hover. */
  tip?: string;
  /** Draw the trailing chevron: the row opens a side panel. */
  chevron?: boolean;
}

/** A row of a `FlyoutMenu`'s main card: an action, or the opener of a side
 *  panel.
 *
 *  Disabled is `aria-disabled`, not the native attribute: a disabled <button>
 *  swallows hover/pointer events in the WebView, so its tooltip would never
 *  show and the user would be left with a dimmed row and no reason. */
export function FlyoutRow({
  icon,
  label,
  note,
  active,
  hot,
  disabled,
  tip,
  chevron,
  className,
  ...rest
}: Props) {
  const cls = [
    "fly-row",
    "flex-center",
    disabled ? "is-disabled" : "",
    tip ? "tip" : "",
    active ? "active" : "",
    hot ? "hot" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" aria-disabled={disabled} className={cls} data-tip={tip} {...rest}>
      {icon}
      <span className="fly-row-name truncate text-base">{label}</span>
      {note !== undefined && <span className="fly-row-note truncate text-xs">{note}</span>}
      {chevron && <Icon name="chevR" size={12} />}
    </button>
  );
}
