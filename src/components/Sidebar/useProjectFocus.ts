import { useCallback, useMemo, useState } from "react";
import { loadProjectFocus, saveProjectFocus } from "@/storage/projectFocus";

/** The sidebar's project focus, persisted in localStorage.
 *
 *  `focus` is the saved keys that still name a project in `allKeys`, so a
 *  removed project — or another environment's project list — reads as no focus
 *  instead of an empty sidebar. The saved value itself is only rewritten by
 *  `setFocus`, never pruned on read: the list is empty while the workspace
 *  loads, and pruning then would forget the focus on every launch. */
export function useProjectFocus(allKeys: readonly string[]) {
  const [saved, setSaved] = useState<string[]>(loadProjectFocus);

  const focus = useMemo(() => {
    const known = new Set(allKeys);
    return saved.filter((k) => known.has(k));
  }, [saved, allKeys]);

  const setFocus = useCallback((keys: string[]) => {
    setSaved(keys);
    saveProjectFocus(keys);
  }, []);

  return { focus, setFocus };
}
