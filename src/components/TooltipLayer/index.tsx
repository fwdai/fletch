import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { placeTooltip } from "./place";

// One tooltip for the whole app. Triggers keep the `tip` class and a `data-tip`
// attribute; this layer watches hover and focus on the document and draws a
// single `position: fixed` bubble at body level. A pseudo-element inside the
// trigger can never escape an ancestor's scroll or overflow: hidden box — this
// can, so the sidebar's right edge and the right panel's top no longer clip.

interface Shown {
  trigger: HTMLElement;
  text: string;
}

function triggerFor(target: EventTarget | null): Shown | null {
  if (!(target instanceof Element)) return null;
  const trigger = target.closest<HTMLElement>("[data-tip]");
  const text = trigger?.dataset.tip?.trim();
  return trigger && text ? { trigger, text } : null;
}

export function TooltipLayer() {
  const [shown, setShown] = useState<Shown | null>(null);
  const bubbleRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const hide = () => setShown(null);
    // `mouseover` fires on whatever the pointer newly entered, so it covers
    // both arriving at a trigger and leaving it for something else.
    const onOver = (e: MouseEvent) => setShown(triggerFor(e.target));
    // Leaving the window: no new element receives a `mouseover`.
    const onOut = (e: MouseEvent) => {
      if (e.relatedTarget === null) hide();
    };
    // Keyboard focus shows the tip; pointer focus (a click) does not — the
    // click already hid it and macOS-style tips stay hidden until re-hover.
    const onFocusIn = (e: FocusEvent) => {
      const next = triggerFor(e.target);
      if (next && next.trigger.matches(":focus-visible")) setShown(next);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };

    document.addEventListener("mouseover", onOver);
    document.addEventListener("mouseout", onOut);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", hide);
    document.addEventListener("mousedown", hide);
    document.addEventListener("keydown", onKey);
    // Any scroll may move the trigger out from under the bubble.
    document.addEventListener("scroll", hide, { capture: true, passive: true });
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    return () => {
      document.removeEventListener("mouseover", onOver);
      document.removeEventListener("mouseout", onOut);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", hide);
      document.removeEventListener("mousedown", hide);
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("scroll", hide, { capture: true });
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
    };
  }, []);

  // Position after render, before paint: the bubble's own size is needed to
  // centre it and to decide whether it fits above the trigger.
  useLayoutEffect(() => {
    const bubble = bubbleRef.current;
    if (!shown || !bubble) return;
    if (!shown.trigger.isConnected) {
      setShown(null);
      return;
    }
    const { top, left, side } = placeTooltip(
      shown.trigger.getBoundingClientRect(),
      bubble.getBoundingClientRect(),
      { width: window.innerWidth, height: window.innerHeight },
    );
    bubble.style.top = `${Math.round(top)}px`;
    bubble.style.left = `${Math.round(left)}px`;
    bubble.dataset.side = side;
  }, [shown]);

  if (!shown) return null;
  return createPortal(
    <div ref={bubbleRef} className="tooltip-bubble" role="tooltip">
      {shown.text}
    </div>,
    document.body,
  );
}
