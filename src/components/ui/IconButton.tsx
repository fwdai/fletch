import type { CSSProperties, MouseEvent, ReactNode } from "react";

interface Props {
  children: ReactNode;
  onClick?: (e: MouseEvent<HTMLButtonElement>) => void;
  /** Standard / small / extra-small. Drives `.btn-i` modifier classes. */
  size?: "md" | "sm" | "xs";
  /** Adds the `active` class — used for "open" affordance on a toggle. */
  active?: boolean;
  /** Text rendered in the CSS-only tooltip above the button. */
  tip?: string;
  /** Anchor the tooltip below the button instead of above. */
  tipDown?: boolean;
  disabled?: boolean;
  "aria-label"?: string;
  className?: string;
  style?: CSSProperties;
  type?: "button" | "submit";
}

/** Square icon button used across title bar, sidebar, composer, and
 *  right panel. Tooltip behavior is CSS-only — set `tip`. */
export function IconButton({
  children,
  onClick,
  size = "md",
  active,
  tip,
  tipDown,
  disabled,
  className,
  style,
  type = "button",
  ...rest
}: Props) {
  const cls = [
    "btn-i",
    "iflex-center",
    size === "sm" ? "sm" : size === "xs" ? "xs" : "",
    active ? "active" : "",
    tip ? "tip" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type={type}
      className={cls}
      onClick={onClick}
      disabled={disabled}
      style={style}
      data-tip={tip}
      data-tip-down={tipDown ? "" : undefined}
      aria-label={rest["aria-label"] ?? tip}
    >
      {children}
    </button>
  );
}
