import { type ActivityStamps, loadStamps, mergeStamps } from "./activityStamps";

const PROJECT_ACTIVITY_KEY = "q2:projectActivity";

/** When a turn last started in each sidebar project, keyed by the project
 *  group id (project_id, or repo path for an unpinned repo) → ms epoch. The
 *  sidebar's "active projects first" order reads it alongside agent launch
 *  times, so resuming an idle workspace counts as activity too. */
export type ProjectActivity = ActivityStamps;

/** Read the saved map. Returns {} on a missing or corrupt value. */
export function loadProjectActivity(): ProjectActivity {
  return loadStamps(PROJECT_ACTIVITY_KEY);
}

/** Merge `stamps` into the saved map, keeping the later time per key, and
 *  persist. Returns the merged map. */
export function stampProjectActivity(stamps: ProjectActivity): ProjectActivity {
  return mergeStamps(PROJECT_ACTIVITY_KEY, stamps);
}
