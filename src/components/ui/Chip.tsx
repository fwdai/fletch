import type { MouseEvent, ReactNode } from "react";

interface Props {
  children: ReactNode;
  onClick?: (e: MouseEvent<HTMLButtonElement>) => void;
  bordered?: boolean;
  tip?: string;
  className?: string;
  disabled?: boolean;
}

/** Composer footer chip — model picker, thinking budget, attach,
 *  base-branch selector. Visual sibling of `IconButton` but with a
 *  text label slot. */
export function Chip({ children, onClick, bordered, tip, className, disabled }: Props) {
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
    <button type="button" className={cls} onClick={onClick} data-tip={tip} disabled={disabled}>
      {children}
    </button>
  );
}
