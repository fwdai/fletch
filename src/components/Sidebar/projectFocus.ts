/** The sidebar's project focus: the group keys the list is narrowed to. An
 *  empty focus is "All projects" — nothing is filtered, and a project added
 *  later shows up without being opted in. */
export type ProjectFocus = readonly string[];

/** `keys` as a focus over the projects there are: in list order (`all` is
 *  every project's key), without keys of projects that are gone. Naming every
 *  project is the same view as no focus at all, so it collapses back to "All
 *  projects" rather than freezing today's project list into the filter. Every
 *  change to the focus goes through this. */
export function normalizeFocus(keys: Iterable<string>, all: readonly string[]): string[] {
  const picked = new Set(keys);
  const ordered = all.filter((k) => picked.has(k));
  return ordered.length === all.length ? [] : ordered;
}

/** The focus once the project `key` joins it — a project just added while
 *  the list is narrowed, which a strict list would otherwise leave out. Null
 *  while there is nothing to do: no focus is on, or the project isn't listed
 *  yet. */
export function joinFocus(
  focus: ProjectFocus,
  key: string,
  all: readonly string[],
): string[] | null {
  if (focus.length === 0 || !all.includes(key)) return null;
  return normalizeFocus([...focus, key], all);
}

/** Add `key` to the focus, or take it out. */
export function toggleFocus(focus: ProjectFocus, key: string, all: readonly string[]): string[] {
  const next = new Set(focus);
  if (!next.delete(key)) next.add(key);
  return normalizeFocus(next, all);
}

/** The groups the focus leaves in view, in their incoming order.
 *
 *  Strict: a project that isn't picked isn't listed, even while a workspace
 *  of it is the one open. What goes on in the projects left out shows as a
 *  mark on the selector instead (see `projectSignal`), so the list is always
 *  exactly what was picked. */
export function applyFocus<G extends { key: string }>(
  groups: readonly G[],
  focus: ProjectFocus,
): G[] {
  if (focus.length === 0) return groups.slice();
  const shown = new Set(focus);
  return groups.filter((g) => shown.has(g.key));
}
