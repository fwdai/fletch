import { type CSSProperties, type ReactNode, useLayoutEffect, useRef, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Scrim } from "@/components/ui/Scrim";

export { FlyoutOption } from "./FlyoutOption";
export { FlyoutRow } from "./FlyoutRow";
export { SideFlyout } from "./SideFlyout";

/** How much of the side panel hides under the card (`FlyoutMenu.css`). */
const TUCK = 16;
/** The least gap a side panel keeps from the window's edge. */
const EDGE = 12;

/** A menu card that opens upward from its trigger, with room for one side
 *  panel sliding out from under it: rows of the card (`FlyoutRow`,
 *  `FlyoutOption`) either act or, hovered, show a `SideFlyout` of choices.
 *
 *  Positioned against the nearest positioned ancestor, which is the trigger's
 *  wrapper: `align` says which of its edges the card lines up with. The side
 *  panel opens to the right, and to the left where only that side of the card
 *  has room for it, the way a submenu does.
 *
 *  The caller owns what is open: which rows there are, which side panel shows
 *  (`flyout`), and when the menu is mounted at all. A click outside or Escape
 *  asks to close. */
export function FlyoutMenu({
  onClose,
  onLeave,
  align = "start",
  sideWidth = 252,
  sideMinHeight,
  flyout,
  children,
}: {
  onClose: () => void;
  /** The pointer left the card and its side panel. */
  onLeave?: () => void;
  /** `start`: the card's left edge on the trigger's. `end`: their right edges,
   *  for a trigger near the right of its row. */
  align?: "start" | "end";
  /** The side panel's width in px, before the window caps it. */
  sideWidth?: number;
  /** The least height of the side panel in px, for a long list beside a short
   *  card. Without it the panel is exactly as tall as the card. */
  sideMinHeight?: number;
  /** The side panel showing, if any. */
  flyout?: ReactNode;
  /** The main card's rows. */
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  // Decided once, as the menu opens: a panel that changed sides while the
  // pointer was on its way to it would be chasing the cursor.
  const [flipped, setFlipped] = useState(false);
  useLayoutEffect(() => {
    const card = ref.current?.getBoundingClientRect();
    if (!card) return;
    const needed = sideWidth - TUCK + EDGE;
    setFlipped(window.innerWidth - card.right < needed && card.left >= needed);
  }, [sideWidth]);

  const cls = [
    "fly-menu",
    align === "end" ? "is-end" : "",
    flipped ? "is-flipped" : "",
    sideMinHeight ? "is-tall" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const vars = {
    "--fly-side-w": `${sideWidth}px`,
    "--fly-side-min-h": sideMinHeight ? `${sideMinHeight}px` : undefined,
  } as CSSProperties;
  return (
    <>
      <Scrim onClose={onClose} />
      {/* Transparent wrapper: the card sits above (z-index 2) the side panel
       *  (z-index 1) so the panel slides out from underneath the card rather
       *  than floating beside it. */}
      <div ref={ref} className={cls} style={vars} onMouseLeave={onLeave}>
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

/** A boxed glyph: the icon of a row that has no avatar or logo of its own. It
 *  lights with its row. `head` is the smaller size a side panel's heading
 *  takes. */
export function FlyoutGlyph({ name, head = false }: { name: IconName; head?: boolean }) {
  return (
    <span className={`fly-glyph ${head ? "is-head" : ""}`}>
      <Icon name={name} size={head ? 13 : 15} />
    </span>
  );
}
