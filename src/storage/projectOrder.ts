// Client-side persistence for the user's manual sidebar project ordering.
// Stored in localStorage (not the Tauri settings db) so reordering needs no
// backend migration — it's a purely cosmetic, per-machine preference keyed by
// repo path. Matches the app's `q2:` localStorage key convention.

const PROJECT_ORDER_KEY = "q2:projectOrder";

/** Read the saved repo-path order. Returns [] on a missing or corrupt value. */
export function loadProjectOrder(): string[] {
  try {
    const raw = localStorage.getItem(PROJECT_ORDER_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}

/** Persist the repo-path order. Write failures (private mode / quota) are ignored. */
export function saveProjectOrder(order: string[]): void {
  try {
    localStorage.setItem(PROJECT_ORDER_KEY, JSON.stringify(order));
  } catch {
    // best-effort — ordering is non-critical cosmetic state
  }
}

/** Rewrite a path in the saved order after a relocate, so the project keeps its
 *  manual position instead of dropping to the bottom under its new path. No-op
 *  when the old path isn't in the saved order. */
export function remapProjectOrder(oldPath: string, newPath: string): void {
  const order = loadProjectOrder();
  if (!order.includes(oldPath)) return;
  saveProjectOrder(order.map((p) => (p === oldPath ? newPath : p)));
}

/** Order `paths` by their index in the saved `order`. Paths absent from `order`
 *  keep their incoming relative order and sort after all known paths, so a
 *  newly-added project appears at the bottom rather than jumping to the top. */
export function sortByOrder(paths: string[], order: string[]): string[] {
  const rank = new Map(order.map((p, i) => [p, i]));
  return paths
    .map((p, i) => ({ p, i }))
    .sort((a, b) => {
      const ra = rank.get(a.p);
      const rb = rank.get(b.p);
      if (ra === undefined && rb === undefined) return a.i - b.i;
      if (ra === undefined) return 1;
      if (rb === undefined) return -1;
      return ra - rb;
    })
    .map((x) => x.p);
}

/** Move `from` to `to`'s position within the already-ordered `visiblePaths`,
 *  returning the new full order. Dragging downward (from before to) drops after
 *  the target; dragging upward drops before it. Returns the input unchanged if
 *  either path is missing or they're identical. */
export function moveInOrder(visiblePaths: string[], from: string, to: string): string[] {
  if (from === to) return visiblePaths;
  const fromIdx = visiblePaths.indexOf(from);
  const toIdx = visiblePaths.indexOf(to);
  if (fromIdx < 0 || toIdx < 0) return visiblePaths;
  const next = visiblePaths.filter((p) => p !== from);
  const targetIdx = next.indexOf(to);
  const insertAt = fromIdx < toIdx ? targetIdx + 1 : targetIdx;
  next.splice(insertAt, 0, from);
  return next;
}
