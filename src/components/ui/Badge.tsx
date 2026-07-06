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
  /** Text for the short CSS-only hover tooltip (single line — see badge.css). */
  tip?: string;
  /** Accessible name for an icon-only badge — renders `role="img"` +
   *  `aria-label` so screen readers announce what the glyph means. */
  label?: string;
  /** Long-form explanation shown as the native `title` tooltip. The OS/webview
   *  positions and wraps it, so unlike `tip` it can't clip at a window edge —
   *  use it for sentence-length copy. Also exposed to assistive tech as the
   *  badge's accessible description. */
  hint?: string;
  className?: string;
}

/** Compact status pill — agent state (new / error) and PR state (open / merged
 *  / closed). Non-interactive (renders a <span>); mono + color-coded. Sibling
 *  of IconButton/Chip. */
export function Badge({ children, variant = "neutral", tip, label, hint, className }: Props) {
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
      title={hint}
      role={label ? "img" : undefined}
      aria-label={label}
    >
      {children}
    </span>
  );
}
