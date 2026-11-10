import { useEffect, useMemo } from "react";
import { useAppStore } from "@/store";
import { joinFocus } from "./projectFocus";

/** The sidebar's project focus, read from the store (which keeps it in
 *  localStorage and lets the palette and ⌘⇧P set it).
 *
 *  `focus` is the saved keys that still name a project in `allKeys`, so a
 *  removed project — or another environment's project list — reads as no focus
 *  instead of an empty sidebar. The saved value itself is only rewritten by
 *  `setFocus`, never pruned on read: the list is empty while the workspace
 *  loads, and pruning then would forget the focus on every launch.
 *
 *  A project added while a focus is on joins it here (the store's `focusJoin`
 *  is the request): `allKeys` is every project the sidebar lists, which is
 *  what deciding takes. Setting the focus clears the request. */
export function useProjectFocus(allKeys: readonly string[]) {
  const saved = useAppStore((s) => s.projectFocus);
  const setFocus = useAppStore((s) => s.setProjectFocus);
  const joining = useAppStore((s) => s.focusJoin);

  const focus = useMemo(() => {
    const known = new Set(allKeys);
    return saved.filter((k) => known.has(k));
  }, [saved, allKeys]);

  useEffect(() => {
    const joined = joining ? joinFocus(focus, joining, allKeys) : null;
    if (joined) setFocus(joined);
  }, [joining, focus, allKeys, setFocus]);

  return { focus, setFocus };
}
