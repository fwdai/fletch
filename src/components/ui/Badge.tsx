import type { ReactNode } from "react";

export type BadgeVariant =
  | "neutral"
  | "new"
  | "err"
  | "docker"
  | "pr-open"
  | "pr-merged"
  | "pr-closed"
  | "pr-pass"
  | "pr-fail";

interface Props {
  children: ReactNode;
  /** Color/tone. Maps to the `.ag-badge` variants in styles/shared/badge.css. */
  variant?: BadgeVariant;
  /** Text for the CSS-only hover tooltip. */
  tip?: string;
  /** Open the tooltip below the badge instead of above — for badges near the
   *  top edge (e.g. the title bar) where an upward tooltip would be clipped. */
  tipDown?: boolean;
  /** Accessible name. Required for icon-only badges: it exposes the meaning to
   *  screen readers (as `role="img"`) and makes the badge keyboard-focusable so
   *  its tooltip is reachable without a pointer. Usually mirrors `tip`. */
  label?: string;
  className?: string;
}

/** Compact status pill — agent state (new / error) and PR state (open / merged
 *  / closed). Non-interactive (renders a <span>); mono + color-coded. Sibling
 *  of IconButton/Chip. */
export function Badge({ children, variant = "neutral", tip, tipDown, label, className }: Props) {
  const cls = [
    "ag-badge",
    "iflex-center",
    variant === "neutral" ? "" : variant,
    tip ? "tip" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <span
      className={cls}
      data-tip={tip}
      data-tip-down={tip && tipDown ? "" : undefined}
      role={label ? "img" : undefined}
      aria-label={label}
      tabIndex={label ? 0 : undefined}
    >
      {children}
    </span>
  );
}
