// What the palette ran lately, by command id, newest first. The empty
// palette's "Recent" section is how a feature someone uses often floats to
// the surface without anyone curating it. Only commands are remembered —
// agents and runs are ranked by what they are doing, not by clicks.

const KEY = "fletch:palette:recents";
export const MAX_RECENTS = 8;

interface Store {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function read(store: Store): string[] {
  try {
    const raw = store.getItem(KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

export function loadRecents(store: Store = localStorage): string[] {
  return read(store).slice(0, MAX_RECENTS);
}

/** Move `id` to the front, keeping the list bounded. Returns the new list. */
export function noteRecent(id: string, store: Store = localStorage): string[] {
  const next = [id, ...read(store).filter((x) => x !== id)].slice(0, MAX_RECENTS);
  try {
    store.setItem(KEY, JSON.stringify(next));
  } catch {
    // Storage full or unavailable: the palette still works, just without memory.
  }
  return next;
}
