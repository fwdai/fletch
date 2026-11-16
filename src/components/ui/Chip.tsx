import type { MouseEvent, ReactNode } from "react";

interface Props {
  children: ReactNode;
  onClick?: (e: MouseEvent<HTMLButtonElement>) => void;
  bordered?: boolean;
  tip?: string;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
  /** For a chip that opens a popover: whether it is open. */
  "aria-expanded"?: boolean;
}

/** Composer footer chip — model picker, thinking budget, attach,
 *  base-branch selector. Visual sibling of `IconButton` but with a
 *  text label slot. */
export function Chip({ children, onClick, bordered, tip, className, disabled, ...aria }: Props) {
  const cls = [
    "c-chip",
    "iflex-center",
    bordered ? "with-border" : "",
    tip ? "tip" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      className={cls}
      onClick={onClick}
      data-tip={tip}
      disabled={disabled}
      aria-label={aria["aria-label"]}
      aria-expanded={aria["aria-expanded"]}
    >
      {children}
    </button>
  );
}
