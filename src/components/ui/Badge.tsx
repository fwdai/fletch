import type { ReactNode } from "react";

export type BadgeVariant =
  | "neutral"
  | "new"
  | "err"
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
  className?: string;
}

/** Compact status pill — agent state (new / error) and PR state (open / merged
 *  / closed). Non-interactive (renders a <span>); mono + color-coded. Sibling
 *  of IconButton/Chip. */
export function Badge({ children, variant = "neutral", tip, className }: Props) {
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
    <span className={cls} data-tip={tip}>
      {children}
    </span>
  );
}
