/** Per-group expanded state, keyed by group id. Absent means "not decided". */
export type OpenMap = Record<string, boolean>;

/** Whether a project group renders expanded.
 *
 *  Outside a search the user's own choices rule, closed by default. While a
 *  query is active every remaining group has matches, so they open by default —
 *  a hit hidden under a collapsed header is invisible, and ↓ from the search
 *  box would find no row to land on. Toggles made mid-search go to a separate
 *  map that is dropped with the query, so the user's collapsed/expanded layout
 *  comes back untouched when the search clears. */
export function isGroupOpen(
  key: string,
  searching: boolean,
  openMap: OpenMap,
  searchOpenMap: OpenMap,
): boolean {
  if (searching) return searchOpenMap[key] ?? true;
  return openMap[key] ?? false;
}
