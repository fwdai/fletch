/** The sidebar's project focus: the group keys the list is narrowed to. An
 *  empty focus is "All projects" — nothing is filtered, and a project added
 *  later shows up without being opted in. */
export type ProjectFocus = readonly string[];

/** Add `key` to the focus, or take it out. `all` is every project's key in
 *  list order, which the result keeps. Ticking the last project left out is
 *  the same view as no focus at all, so it collapses back to "All projects"
 *  rather than freezing today's project list into the filter. */
export function toggleFocus(focus: ProjectFocus, key: string, all: readonly string[]): string[] {
  const next = new Set(focus);
  if (!next.delete(key)) next.add(key);
  const ordered = all.filter((k) => next.has(k));
  return ordered.length === all.length ? [] : ordered;
}

/** The groups the focus leaves in view, in their incoming order.
 *
 *  The group holding the open workspace (`selectedKey`) stays whatever the
 *  focus says — the same rule that keeps the selected row out of the "older"
 *  fold. Jumping to a workspace through the palette or a notification, or
 *  adding a project, must not land on a sidebar that doesn't show where you
 *  are. */
export function applyFocus<G extends { key: string }>(
  groups: readonly G[],
  focus: ProjectFocus,
  selectedKey: string | null,
): G[] {
  if (focus.length === 0) return groups.slice();
  const shown = new Set(focus);
  return groups.filter((g) => shown.has(g.key) || g.key === selectedKey);
}
