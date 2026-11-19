// A scrolling log that follows its bottom: new output, a row growing, the
// composer under it growing a line all keep the last line in view, until the
// user scrolls up. One rule for every conversation surface.

import { type RefObject, useCallback, useLayoutEffect, useRef } from "react";

/** How close to the bottom a scroll down has to come to take up following. */
const NEAR = 40;

const LEAVE = "stick:leave";

/** Whether the scroller is following its bottom, given where a scroll event
 *  found it. Scrolling up lets go; scrolling down to within `NEAR` of the
 *  bottom, or landing on it (a clamp when the content shrinks), follows. */
export function followsAfterScroll(
  following: boolean,
  top: number,
  lastTop: number,
  gap: number,
): boolean {
  if (gap <= 1 || (top > lastTop && gap <= NEAR)) return true;
  return top < lastTop ? false : following;
}

/** Keep `ref`'s scroller at its bottom while the user is there. It follows the
 *  rendered size of the scroller and of its one child (the content column), so
 *  whatever changes either is covered, in the frame it happens. `resetKey`
 *  starts over at the bottom (another conversation in the same scroller).
 *  Returns `toBottom`, for a send. */
export function useStickToBottom(
  ref: RefObject<HTMLElement | null>,
  resetKey?: unknown,
): () => void {
  const toBottom = useRef(() => {});

  // biome-ignore lint/correctness/useExhaustiveDependencies: `resetKey` is a signal to start over, not a value the body reads
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let following = true;
    let lastTop = el.scrollTop;
    let lastHeight = el.scrollHeight;
    const gap = () => el.scrollHeight - el.scrollTop - el.clientHeight;

    const follow = () => {
      following = true;
      el.scrollTop = el.scrollHeight;
      // Remembered, so the scroll event this write causes isn't read as the
      // user's, and a scroll up from here is seen as one however much the
      // content grew in between.
      lastTop = el.scrollTop;
    };
    const onScroll = () => {
      const top = el.scrollTop;
      if (top === lastTop) return;
      following = followsAfterScroll(following, top, lastTop, gap());
      lastTop = top;
    };
    const onResize = () => {
      // Content that shrank back to the bottom (a row the user closed) fires
      // no scroll event, so following is taken up again here.
      const shrankToBottom = el.scrollHeight < lastHeight && gap() <= 1;
      lastHeight = el.scrollHeight;
      if (following || shrankToBottom) follow();
    };
    const onLeave = (e: Event) => {
      const block = (e as CustomEvent<ScrollLogicalPosition | undefined>).detail;
      // A jump lets go only when it takes the view up. One to a row already
      // on screen at the bottom moves nothing, and the log goes on following.
      if (block && !movesUp(el, e.target as Element, block)) return;
      following = false;
    };

    toBottom.current = follow;
    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener(LEAVE, onLeave);
    const ro = new ResizeObserver(onResize);
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener(LEAVE, onLeave);
      ro.disconnect();
    };
  }, [ref, resetKey]);

  return useCallback(() => toBottom.current(), []);
}

function movesUp(el: HTMLElement, row: Element, block: ScrollLogicalPosition): boolean {
  const r = row.getBoundingClientRect();
  const view = el.getBoundingClientRect();
  const offset =
    block === "center" ? (r.top + r.bottom - view.top - view.bottom) / 2 : r.top - view.top;
  return offset < 0 && el.scrollTop > 0;
}

/** `scrollIntoView` for a row of a log that may be following its bottom: the
 *  log lets go first, or its next follow would cancel the jump. */
export function scrollToRow(row: Element, opts: ScrollIntoViewOptions): void {
  row.dispatchEvent(new CustomEvent(LEAVE, { bubbles: true, detail: opts.block ?? "start" }));
  row.scrollIntoView(opts);
}

/** Hold the log where it is: the user is reading here (they opened a row, a
 *  find landed above), so it stops following until they return to the bottom. */
export function holdView(from: Element): void {
  from.dispatchEvent(new CustomEvent(LEAVE, { bubbles: true }));
}
