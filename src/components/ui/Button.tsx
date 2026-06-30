import type { CSSProperties, MouseEvent, ReactNode } from "react";

export type ButtonVariant = "ghost" | "outline" | "primary";

interface Props {
  children: ReactNode;
  /** Visual style. Maps to the `.btn-t` variant classes in icon-button.css. */
  variant: ButtonVariant;
  /** Danger tint — composes with `ghost`/`outline`. */
  danger?: boolean;
  /** `sm` applies the compact `.sm-t` sizing. */
  size?: "md" | "sm";
  onClick?: (e: MouseEvent<HTMLButtonElement>) => void;
  /** Text for the CSS-only hover tooltip. */
  tip?: string;
  disabled?: boolean;
  className?: string;
  style?: CSSProperties;
  type?: "button" | "submit";
}

/** Text-label button — CTAs and dialog actions (Cancel / Save / Restart …).
 *  Sibling of `IconButton`, which handles icon-only buttons. */
export function Button({
  children,
  variant,
  danger,
  size = "md",
  onClick,
  tip,
  disabled,
  className,
  style,
  type = "button",
}: Props) {
  const cls = [
    "btn-t",
    "iflex-center",
    variant,
    danger ? "danger" : "",
    size === "sm" ? "sm-t" : "",
    tip ? "tip" : "",
    className,
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
    >
      {children}
    </button>
  );
}
