// Keyboard stepping through the sidebar's selectable rows (drafts, agents,
// workflow runs). Read from the DOM on each keypress rather than mirrored in
// state: the rendered rows already reflect search filtering, project order,
// and collapsed groups, so there is nothing to keep in sync.

const ROW = ".agent[role='button']";

/** Every selectable row the user can currently see, in visual order. Rows in a
 *  collapsed group stay mounted (the group animates shut), so they're skipped. */
export function visibleRows(container: HTMLElement | null): HTMLElement[] {
  return Array.from(container?.querySelectorAll<HTMLElement>(ROW) ?? []).filter(
    (el) => !el.closest(".agents.closed"),
  );
}

/** The row that owns `el`, if it sits inside one (a nested Stop/Archive button
 *  counts as being on its row). */
export function rowOf(el: EventTarget | null): HTMLElement | null {
  return el instanceof HTMLElement ? el.closest<HTMLElement>(ROW) : null;
}

/** Move keyboard focus to a row and select it, so the transcript follows the
 *  arrow keys the way a mail or file list does. Clicking reuses the row's own
 *  onClick, so drafts, agents, and runs all route through their usual select. */
export function focusRow(row: HTMLElement) {
  row.focus();
  row.scrollIntoView({ block: "nearest" });
  row.click();
}
