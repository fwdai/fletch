// Keyboard stepping through the sidebar's selectable rows (drafts, agents,
// workflow runs). Read from the DOM on each keypress rather than mirrored in
// state: the rendered rows already reflect search filtering, project order,
// and collapsed groups, so there is nothing to keep in sync.

/** A stop: a draft, agent or run row. */
const ROW = ".agent[role='button']";

/** Every selectable row the user can currently see, in visual order. Rows in a
 *  collapsed group stay mounted (the group animates shut), so they're skipped. */
export function visibleRows(container: HTMLElement | null): HTMLElement[] {
  return Array.from(container?.querySelectorAll<HTMLElement>(ROW) ?? []).filter(
    (el) => !el.closest(".agents.closed"),
  );
}

/** Where an arrow key pressed on `el` steps from, as an index into `rows`: the
 *  row that owns `el` (a nested Stop/Archive button counts as being on its
 *  row), or, from a control between rows that is no stop itself (the "N older"
 *  fold), its place among them, so ↓ lands on the row below and ↑ on the row
 *  above. */
export function stepOrigin(rows: HTMLElement[], el: EventTarget | null, key: string): number {
  if (!(el instanceof HTMLElement)) return -1;
  const row = el.closest<HTMLElement>(ROW);
  if (row) return rows.indexOf(row);
  const above = rows.filter(
    (r) => r.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).length;
  return key === "ArrowUp" ? above : above - 1;
}

/** Move keyboard focus to a row and select it, so the transcript follows the
 *  arrow keys the way a mail or file list does. Clicking reuses the row's own
 *  onClick, so drafts, agents, and runs all route through their usual select. */
export function focusRow(row: HTMLElement) {
  row.focus();
  row.scrollIntoView({ block: "nearest" });
  row.click();
}

/** The list the rows live in; mounted with the sidebar. */
export const SIDEBAR_LIST_ID = "sidebar-list";

/** Select the row after (or before) the selected one, wrapping at the ends,
 *  without moving keyboard focus — the whole-window shortcut's version of the
 *  arrow keys, over the very same rows, so the two can never disagree about
 *  what "next" is. Nothing selected steps in from the top (or bottom). Does
 *  nothing while the sidebar is unmounted; the caller reveals it first. */
export function stepSelection(dir: 1 | -1) {
  const rows = visibleRows(document.getElementById(SIDEBAR_LIST_ID));
  if (rows.length === 0) return;
  const current = rows.findIndex((r) => r.classList.contains("active"));
  const next =
    current < 0
      ? rows[dir > 0 ? 0 : rows.length - 1]
      : rows[(current + dir + rows.length) % rows.length];
  next.scrollIntoView({ block: "nearest" });
  next.click();
}
