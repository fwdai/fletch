// A list of strings kept in localStorage: the shape the sidebar's cosmetic,
// per-machine preferences share (project order, project focus).

/** Read the list saved under `key`. Returns [] on a missing or corrupt value. */
export function loadStringList(key: string): string[] {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}

/** Persist `list` under `key`. Write failures (private mode / quota) are
 *  ignored: what is kept this way is never critical. */
export function saveStringList(key: string, list: readonly string[]): void {
  try {
    localStorage.setItem(key, JSON.stringify(list));
  } catch {}
}
