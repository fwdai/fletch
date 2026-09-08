import { type MutableRefObject, useCallback, useLayoutEffect, useRef } from "react";

/** How many pixels short of the bottom still count as "at the bottom".
 *  Sub-pixel rounding makes exact equality flaky; the desktop transcript
 *  allows the same slop. */
const BOTTOM_SLOP = 40;

/** Keep a scroll container pinned to its bottom as content arrives, and stop
 *  following the moment the user scrolls up — the desktop transcript's
 *  behaviour (`TranscriptList`).
 *
 *  `deps` are change signals, not values the effect reads: pass whatever
 *  identifies "new content" (a log array, a status). They have to change on
 *  *every* update that grows the container, including a streaming message
 *  extended in place — which is why this can't key off a rendered item count.
 *
 *  Pass `pinRef` to own the pin flag in a parent, so a composer that sends a
 *  message can re-pin the log the way the desktop's `ChatView` does. */
export function useStickyScroll<T extends HTMLElement>(
  deps: unknown[],
  pinRef?: MutableRefObject<boolean>,
) {
  const ref = useRef<T>(null);
  const ownPin = useRef(true);
  const pinned = pinRef ?? ownPin;

  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_SLOP;
  }, [pinned]);

  // A freshly mounted scroller opens at its latest, so a lifted `pinRef` can't
  // carry a stale `false` into it — scroll up in the chat, switch to Code and
  // back, and the new scroller would otherwise sit at the top and never
  // follow. The desktop re-pins on the same reasoning when you switch agents.
  // Declared before the follow effect so it has already run when that one
  // does its initial scroll.
  useLayoutEffect(() => {
    pinned.current = true;
  }, [pinned]);

  // A layout effect, so the jump happens before the browser paints the content
  // that grew — a plain effect shows one frame at the old offset.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !pinned.current) return;
    el.scrollTop = el.scrollHeight;
  }, [...deps, pinned]);

  return { ref, onScroll };
}
