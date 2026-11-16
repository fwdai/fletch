import { useEffect, useRef, useState } from "react";

/** How long a fetched list stays fresh before the next open refetches — long
 *  enough to absorb rapid open/close cycling, short enough to pick up what
 *  was opened elsewhere within a session. */
const CACHE_MS = 15_000;

/** A list fetched when its menu opens and kept for a short while, so reopening
 *  it (type `#`, delete, retype; hover a row, leave, hover again) doesn't
 *  queue a burst of fetches. Null until the first answer; a failed first
 *  fetch reads as empty, a failed refresh keeps what was there.
 *
 *  An answer lands whether or not the menu is still open: the fetch that
 *  stamped the cache window is the only one made inside it, so dropping its
 *  answer when the menu closed would leave a quick reopen with nothing.
 *
 *  `list` is read through a ref, so an inline function doesn't refetch; omit
 *  it where there is nothing to list. */
export function useCachedList<T>(
  list: (() => Promise<T[]>) | undefined,
  open: boolean,
): T[] | null {
  const [items, setItems] = useState<T[] | null>(null);
  const ref = useRef(list);
  ref.current = list;
  // Stamped before the call, so a reopen while one is in flight skips too.
  const lastFetch = useRef(0);
  useEffect(() => {
    if (!open || !ref.current) return;
    if (Date.now() - lastFetch.current < CACHE_MS) return;
    lastFetch.current = Date.now();
    ref.current().then(
      (next) => setItems(next),
      () => setItems((cur) => cur ?? []),
    );
  }, [open]);
  return items;
}
