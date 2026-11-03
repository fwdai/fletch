import type { MouseEvent, ReactNode } from "react";
import { Icon } from "@/components/Icon";

/** The composer's run rail: one framed strip holding every control that
 *  decides how the agent runs, read left to right as a chain — where it runs
 *  › which agent › under which account › which model › at what effort. Each
 *  piece is its own control with its own menu; the frame is shared so the row
 *  reads as one setting rather than a scatter of chips.
 *
 *  Groups (`RailGroup`) part with a hairline and anchor their own menus; the
 *  parts of the agent chain inside its one piece part with a `›`
 *  (`RailLink`). */
export function RunRail({ children }: { children: ReactNode }) {
  return <div className="run-rail">{children}</div>;
}

/** One group of the rail — the anchor its menu is positioned against. */
export function RailGroup({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={["run-group", className].filter(Boolean).join(" ")}>{children}</div>;
}

/** A piece of the rail: a button that opens a menu. `open` marks it while
 *  its menu shows. The tooltip is the `.tip`/`data-tip` one. */
export function RailPiece({
  children,
  tip,
  open = false,
  glyph = false,
  className,
  onClick,
}: {
  children: ReactNode;
  tip?: string;
  open?: boolean;
  /** An icon-only piece, padded to sit square. */
  glyph?: boolean;
  className?: string;
  onClick?: (e: MouseEvent<HTMLButtonElement>) => void;
}) {
  const cls = [
    "run-piece",
    "iflex-center",
    open ? "is-open" : "",
    glyph ? "is-glyph" : "",
    tip ? "tip" : "",
    className ?? "",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      className={cls}
      data-tip={tip}
      aria-haspopup="menu"
      aria-expanded={open}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

/** The `›` that chains one part of a piece to the next, with the part it
 *  leads to — hidden together where the row is too narrow for it. */
export function RailLink({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={["run-link", "iflex-center", className].filter(Boolean).join(" ")}>
      <Icon name="chevR" size={10} className="run-chev" />
      {children}
    </span>
  );
}
