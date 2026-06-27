import { useCallback, useEffect, useRef, useState } from "react";

/** Names of the two registered CSS custom highlights. One paints every match
 *  faintly; the other paints the active match in the accent colour. */
const HL_ALL = "chat-find";
const HL_CURRENT = "chat-find-current";

/** The CSS Custom Highlight API lets us paint matches over the existing text
 *  nodes via Range objects — without mutating the DOM that React owns, so we
 *  never fight re-renders or break the markdown tree. Feature-detected because
 *  it only landed in WebKit 17.2; where missing, navigation still works (we
 *  scroll to each match) but nothing is painted. */
const HIGHLIGHT_SUPPORTED =
  typeof CSS !== "undefined" &&
  "highlights" in CSS &&
  typeof (globalThis as { Highlight?: unknown }).Highlight !== "undefined";

export interface ChatSearchState {
  /** Total number of matches currently in the (visible) conversation. */
  total: number;
  /** 1-based index of the active match for display; 0 when there are none. */
  current: number;
  /** Advance to the next match, wrapping around at the end. */
  next: () => void;
  /** Step to the previous match, wrapping around at the start. */
  prev: () => void;
}

/** Walk every visible text node under `container` and collect a Range for each
 *  case-insensitive occurrence of `query`. Matches are returned in document
 *  order, which is also visual (top-to-bottom) order in this scroll log. */
function collectRanges(container: HTMLElement, query: string): Range[] {
  const ranges: Range[] = [];
  const needle = query.toLowerCase();
  if (!needle) return ranges;

  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      return node.nodeValue && node.nodeValue.length > 0
        ? NodeFilter.FILTER_ACCEPT
        : NodeFilter.FILTER_REJECT;
    },
  });

  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const haystack = (node.nodeValue ?? "").toLowerCase();
    let from = haystack.indexOf(needle);
    while (from !== -1) {
      const range = document.createRange();
      range.setStart(node, from);
      range.setEnd(node, from + needle.length);
      ranges.push(range);
      from = haystack.indexOf(needle, from + needle.length);
    }
  }
  return ranges;
}

/** Scroll the container so `range` is comfortably visible. Leaves it alone when
 *  already fully in view, so typing more characters doesn't yank a still-visible
 *  match around; otherwise centres it. */
function scrollRangeIntoView(container: HTMLElement, range: Range): void {
  const rRect = range.getBoundingClientRect();
  if (rRect.width === 0 && rRect.height === 0) return; // detached / zero-size
  const cRect = container.getBoundingClientRect();
  if (rRect.top >= cRect.top && rRect.bottom <= cRect.bottom) return; // visible
  const offset = rRect.top - cRect.top - container.clientHeight / 2 + rRect.height / 2;
  container.scrollTo({ top: container.scrollTop + offset, behavior: "smooth" });
}

function clearHighlights(): void {
  if (!HIGHLIGHT_SUPPORTED) return;
  CSS.highlights.delete(HL_ALL);
  CSS.highlights.delete(HL_CURRENT);
}

/** In-conversation find. Recomputes match ranges whenever the query, the
 *  conversation content, or the active flag changes, and exposes next/prev
 *  navigation that paints + scrolls to the active match.
 *
 *  @param containerRef   the scrollable chat log element to search within
 *  @param query          the search text (empty clears everything)
 *  @param active         whether the search bar is open
 *  @param contentVersion any value whose identity changes when the rendered
 *                        conversation changes (e.g. the derived items array),
 *                        used to recompute matches as messages stream in */
export function useChatSearch(
  containerRef: React.RefObject<HTMLElement | null>,
  query: string,
  active: boolean,
  contentVersion: unknown,
): ChatSearchState {
  const [total, setTotal] = useState(0);
  const [idx, setIdx] = useState(0);
  const rangesRef = useRef<Range[]>([]);

  // Recompute the match set. Resetting idx to 0 keeps "type, then Enter to walk
  // matches" behaving like every other find box.
  useEffect(() => {
    const container = containerRef.current;
    if (!active || !container || !query) {
      rangesRef.current = [];
      setTotal(0);
      setIdx(0);
      clearHighlights();
      return;
    }
    const ranges = collectRanges(container, query);
    rangesRef.current = ranges;
    setTotal(ranges.length);
    setIdx(0);
  }, [containerRef, query, active, contentVersion]);

  // Paint the current match set and bring the active one into view. Runs after
  // the recompute effect (idx/total just changed) and on every next/prev.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ranges = rangesRef.current;
    if (HIGHLIGHT_SUPPORTED) {
      clearHighlights();
      if (ranges.length > 0) {
        CSS.highlights.set(HL_ALL, new Highlight(...ranges));
        const cur = ranges[idx];
        if (cur) CSS.highlights.set(HL_CURRENT, new Highlight(cur));
      }
    }
    const cur = ranges[idx];
    if (cur) scrollRangeIntoView(container, cur);
  }, [containerRef, idx, total]);

  // Always drop highlights when the hook unmounts (search closed / view gone).
  useEffect(() => clearHighlights, []);

  const next = useCallback(() => {
    setIdx((i) => {
      const n = rangesRef.current.length;
      return n === 0 ? 0 : (i + 1) % n;
    });
  }, []);

  const prev = useCallback(() => {
    setIdx((i) => {
      const n = rangesRef.current.length;
      return n === 0 ? 0 : (i - 1 + n) % n;
    });
  }, []);

  return { total, current: total === 0 ? 0 : idx + 1, next, prev };
}
