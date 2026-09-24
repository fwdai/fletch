const PROJECT_ACTIVITY_KEY = "q2:projectActivity";

/** When a turn last started in each sidebar project, keyed by the project
 *  group id (project_id, or repo path for an unpinned repo) → ms epoch. The
 *  sidebar's "active projects first" order reads it alongside agent launch
 *  times, so resuming an idle workspace counts as activity too. */
export type ProjectActivity = Record<string, number>;

/** Read the saved map. Returns {} on a missing or corrupt value. */
export function loadProjectActivity(): ProjectActivity {
  try {
    const raw = localStorage.getItem(PROJECT_ACTIVITY_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: ProjectActivity = {};
    for (const [k, v] of Object.entries(parsed)) if (typeof v === "number") out[k] = v;
    return out;
  } catch {
    return {};
  }
}

/** Merge `stamps` into the saved map, keeping the later time per key, and
 *  persist. Returns the merged map so a caller holding it in state can adopt
 *  the result directly. Write failures (private mode / quota) are ignored. */
export function stampProjectActivity(stamps: ProjectActivity): ProjectActivity {
  const next = loadProjectActivity();
  for (const [k, t] of Object.entries(stamps)) if (t > (next[k] ?? 0)) next[k] = t;
  try {
    localStorage.setItem(PROJECT_ACTIVITY_KEY, JSON.stringify(next));
  } catch {}
  return next;
}
