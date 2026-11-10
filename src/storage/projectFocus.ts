// Which projects the sidebar is narrowed to, kept in localStorage like the
// manual project order: a cosmetic, per-machine view preference.

import { loadStringList, saveStringList } from "./localList";

const PROJECT_FOCUS_KEY = "q2:projectFocus";

/** The saved focus: sidebar group keys (a project id, or a repo path for an
 *  unpinned repo). Empty means every project is shown. */
export function loadProjectFocus(): string[] {
  return loadStringList(PROJECT_FOCUS_KEY);
}

export function saveProjectFocus(keys: readonly string[]): void {
  saveStringList(PROJECT_FOCUS_KEY, keys);
}
