/** Resolve where an ArrowUp/ArrowDown/Home/End keypress should move within an
 *  ordered list of items. Pure — callers decide what "focus" means. `current`
 *  is the index of the item that has focus now, or -1 when focus sits outside
 *  the list (then ArrowDown lands on the first item). Returns undefined for
 *  keys the helper doesn't handle, so callers can let them through. */
export function arrowTarget<T>(
  items: readonly T[],
  current: number,
  key: string,
  opts: { wrap?: boolean } = {},
): T | undefined {
  const n = items.length;
  if (n === 0) return undefined;
  switch (key) {
    case "ArrowDown":
      return opts.wrap ? items[(current + 1) % n] : items[Math.min(current + 1, n - 1)];
    case "ArrowUp":
      return opts.wrap ? items[(current - 1 + n) % n] : items[Math.max(current - 1, 0)];
    case "Home":
      return items[0];
    case "End":
      return items[n - 1];
    default:
      return undefined;
  }
}
