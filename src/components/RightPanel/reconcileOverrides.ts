import type { SetupRow } from "./RunSettingsSheet";

/** The outcome of reconciling a draft against the detected rows. */
export interface OverrideReconciliation {
  /** Override map to keep in component state (live + differing-from-default). */
  cleaned: Record<string, string>;
  /** Keys whose DB value should be upserted. */
  toSet: Array<{ id: string; value: string }>;
  /** Keys whose DB value should be deleted. */
  toDelete: string[];
}

/**
 * Decide which run-config overrides to keep, persist, and delete.
 *
 * A value is a real override only when it has a matching detected row AND
 * differs from that row's detected default. Everything else is dropped:
 * values that match the default, and — crucially — keys that no longer
 * correspond to any detected row (stale entries left behind when the
 * project changes ecosystem). Without pruning those, they accumulate in
 * the DB and keep the override indicator lit with no way to clear them.
 *
 * The key set is the union of the current rows, the previously persisted
 * overrides, and the draft — so disappeared rows are still reconciled.
 */
export function reconcileOverrides(
  rows: SetupRow[],
  previous: Record<string, string>,
  next: Record<string, string>,
): OverrideReconciliation {
  const ids = new Set<string>([
    ...rows.map((r) => r.id),
    ...Object.keys(previous),
    ...Object.keys(next),
  ]);

  const cleaned: Record<string, string> = {};
  const toSet: Array<{ id: string; value: string }> = [];
  const toDelete: string[] = [];

  for (const id of ids) {
    const row = rows.find((r) => r.id === id);
    const nextVal = next[id];
    const isOverride = row !== undefined && nextVal !== undefined && nextVal !== row.value;

    if (isOverride) {
      cleaned[id] = nextVal;
      if (previous[id] !== nextVal) {
        toSet.push({ id, value: nextVal });
      }
    } else if (previous[id] !== undefined) {
      toDelete.push(id);
    }
  }

  return { cleaned, toSet, toDelete };
}
