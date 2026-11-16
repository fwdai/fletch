import type { ReactNode } from "react";
import { Scrim } from "@/components/ui/Scrim";

export { FlyoutOption } from "./FlyoutOption";
export { FlyoutRow } from "./FlyoutRow";
export { SideFlyout } from "./SideFlyout";

/** A menu card that opens upward from its trigger, with room for one side
 *  panel sliding out from under it: rows of the card (`FlyoutRow`,
 *  `FlyoutOption`) either act or, hovered, show a `SideFlyout` of choices.
 *
 *  Positioned against the nearest positioned ancestor, which is the trigger's
 *  wrapper. The caller owns what is open: which rows there are, which side
 *  panel shows (`flyout`), and when the menu is mounted at all. A click outside
 *  or Escape asks to close. */
export function FlyoutMenu({
  onClose,
  onLeave,
  flyout,
  children,
}: {
  onClose: () => void;
  /** The pointer left the card and its side panel. */
  onLeave?: () => void;
  /** The side panel showing, if any. */
  flyout?: ReactNode;
  /** The main card's rows. */
  children: ReactNode;
}) {
  return (
    <>
      <Scrim onClose={onClose} />
      {/* Transparent wrapper: the card sits above (z-index 2) the side panel
       *  (z-index 1) so the panel slides out from underneath the card rather
       *  than floating beside it. */}
      <div className="fly-menu" onMouseLeave={onLeave}>
        <div className="fly-menu-card">{children}</div>
        {flyout}
      </div>
    </>
  );
}

/** A section heading of the card, with a rule trailing to its edge. */
export function FlyoutSection({ children }: { children: ReactNode }) {
  return (
    <div className="fly-sect flex-center text-xs">
      <span>{children}</span>
      <span className="fly-sect-line" />
    </div>
  );
}
