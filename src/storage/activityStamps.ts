// "When was this last used" maps kept in localStorage: an id → ms epoch, one
// map per storage key. The sidebar orders projects and folds workspaces by
// them; the host records launches but not turns, so the turns live here.

export type ActivityStamps = Record<string, number>;

/** Read the map saved under `storageKey`. Returns {} on a missing or corrupt
 *  value. */
export function loadStamps(storageKey: string): ActivityStamps {
  try {
    const raw = localStorage.getItem(storageKey);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: ActivityStamps = {};
    for (const [k, v] of Object.entries(parsed)) if (typeof v === "number") out[k] = v;
    return out;
  } catch {
    return {};
  }
}

/** Merge `stamps` into the saved map, keeping the later time per id, and
 *  persist. Entries older than `keepSince` are dropped, for a map whose ids
 *  come and go. Returns the merged map so a caller holding it in state can
 *  adopt the result directly. Write failures (private mode / quota) are
 *  ignored. */
export function mergeStamps(
  storageKey: string,
  stamps: ActivityStamps,
  keepSince = 0,
): ActivityStamps {
  const next = loadStamps(storageKey);
  for (const [k, t] of Object.entries(stamps)) if (t > (next[k] ?? 0)) next[k] = t;
  for (const [k, t] of Object.entries(next)) if (t < keepSince) delete next[k];
  try {
    localStorage.setItem(storageKey, JSON.stringify(next));
  } catch {}
  return next;
}
