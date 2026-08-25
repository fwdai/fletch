// ThreadView/useThreadScroll.ts — bottom-pinning for a scroll container whose
// content comes from several sources at once.
//
// TranscriptList can follow one agent's log by watching that log's identity. The
// thread has no single such signal: segments append, a lazily-loaded history
// lands under an older segment, and a phase row swaps for a taller one. So it
// follows the rendered height instead — one ResizeObserver on the inner column
// covers every one of those cases with the same rule.

import { type MutableRefObject, useEffect, useRef } from "react";

/** Slop that still counts as "at the bottom" — sub-pixel rounding otherwise
 *  makes exact equality flaky. Matches TranscriptList. */
const BOTTOM_SLOP = 40;

export interface ThreadScroll {
  scrollRef: MutableRefObject<HTMLDivElement | null>;
  innerRef: MutableRefObject<HTMLDivElement | null>;
  /** True while the thread follows new content; false once the user scrolls up. */
  pinned: MutableRefObject<boolean>;
  onScroll: () => void;
  /** Re-pin and jump to the bottom (used when the user sends a message). */
  toBottom: () => void;
}

export function useThreadScroll(runId: string): ThreadScroll {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const innerRef = useRef<HTMLDivElement | null>(null);
  const pinned = useRef(true);

  // Each run opens at its latest.
  useEffect(() => {
    pinned.current = true;
  }, [runId]);

  useEffect(() => {
    const el = scrollRef.current;
    const inner = innerRef.current;
    if (!el || !inner) return;
    const follow = () => {
      if (pinned.current) el.scrollTop = el.scrollHeight;
    };
    follow();
    const ro = new ResizeObserver(follow);
    ro.observe(inner);
    return () => ro.disconnect();
  }, []);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_SLOP;
  };

  const toBottom = () => {
    pinned.current = true;
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  };

  return { scrollRef, innerRef, pinned, onScroll, toBottom };
}
