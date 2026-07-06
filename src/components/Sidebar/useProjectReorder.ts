import { useCallback, useState } from "react";
import {
  loadProjectOrder,
  moveInOrder,
  saveProjectOrder,
  sortByOrder,
} from "@/storage/projectOrder";

/** Manual, localStorage-persisted ordering for sidebar project groups.
 *
 *  `sortPaths` orders a set of repo paths by the saved order; paths absent from
 *  it keep their natural order and sort last, so newly-added projects append at
 *  the bottom. `reorder` moves one path relative to another and persists the
 *  resulting full order. */
export function useProjectReorder() {
  const [order, setOrder] = useState<string[]>(loadProjectOrder);

  const sortPaths = useCallback((paths: string[]) => sortByOrder(paths, order), [order]);

  const reorder = useCallback((visiblePaths: string[], from: string, to: string) => {
    const next = moveInOrder(visiblePaths, from, to);
    setOrder(next);
    saveProjectOrder(next);
  }, []);

  return { sortPaths, reorder };
}
