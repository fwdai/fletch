import type { ComponentPropsWithoutRef, RefObject } from "react";
import { useLayoutEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { placeTooltip } from "@/components/TooltipLayer/place";

interface AnchoredPopoverProps extends ComponentPropsWithoutRef<"div"> {
  /** The element the card hangs off. It has to be mounted before the card is,
   *  as it is for a card opened by hovering or clicking it. */
  anchorRef: RefObject<HTMLElement | null>;
}

/** A floating card beside `anchorRef`'s element: below it with their left
 *  edges aligned, above when only that side has room, and slid sideways to
 *  stay on screen. Drawn at body level because a card inside the anchor is
 *  clipped by any scrolling ancestor (the sidebar's list, a panel), the reason
 *  the tooltip is a layer too. It follows the anchor through scrolls and
 *  resizes, and is placed again when its own size changes. The caller owns
 *  when it is mounted and what it looks like. */
export function AnchoredPopover({ anchorRef, style, ...rest }: AnchoredPopoverProps) {
  const cardRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const card = cardRef.current;
    if (!anchor || !card) return;
    const place = () => {
      const { top, left } = placeTooltip(
        anchor.getBoundingClientRect(),
        // Layout size: an entrance animation's transform must not skew it.
        { width: card.offsetWidth, height: card.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
        { prefer: "below", align: "start" },
      );
      card.style.top = `${Math.round(top)}px`;
      card.style.left = `${Math.round(left)}px`;
    };
    place();
    // Content that arrives while the card is open (a failure reason, a model
    // name) changes its height, and with it which side it fits on.
    const resized = new ResizeObserver(place);
    resized.observe(card);
    document.addEventListener("scroll", place, { capture: true, passive: true });
    window.addEventListener("resize", place);
    return () => {
      resized.disconnect();
      document.removeEventListener("scroll", place, { capture: true });
      window.removeEventListener("resize", place);
    };
  }, [anchorRef]);

  return createPortal(
    <div ref={cardRef} style={{ ...style, position: "fixed" }} {...rest} />,
    document.body,
  );
}
