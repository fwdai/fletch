// Which changed files the user has read. A file is remembered with the +/−
// counts it had when read, and counts as viewed only while it still has them:
// an edit after viewing un-views it, with no event needed to say so.
import { useCallback, useEffect, useMemo, useState } from "react";
import type { DiffBaseMode, FileStatus } from "@/api";

/** path → the "adds:dels" signature the file had when it was viewed. */
export type ViewedMap = Record<string, string>;

export const signatureOf = (f: FileStatus) => `${f.additions}:${f.deletions}`;

export const isViewedIn = (map: ViewedMap, f: FileStatus) => map[f.path] === signatureOf(f);

/** `map` with each of `paths` recorded at its current signature in `files`.
 *  Paths not in `files` have no signature to record and are skipped. */
export function withViewed(map: ViewedMap, files: FileStatus[], paths: string[]): ViewedMap {
  const wanted = new Set(paths);
  const next = { ...map };
  for (const f of files) if (wanted.has(f.path)) next[f.path] = signatureOf(f);
  return next;
}

/** `map` with `f` flipped: unviewed if it is viewed now, else viewed. */
export function withToggled(map: ViewedMap, f: FileStatus): ViewedMap {
  if (!isViewedIn(map, f)) return { ...map, [f.path]: signatureOf(f) };
  const { [f.path]: _, ...rest } = map;
  return rest;
}

// Per agent and per scope: the counts are measured against the scope's base,
// so a signature from "All" means nothing in "Uncommitted".
const storageKey = (agentId: string, base: DiffBaseMode) => `q2:viewed:${agentId}:${base}`;

function load(key: string): ViewedMap {
  try {
    const parsed = JSON.parse(localStorage.getItem(key) ?? "{}");
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

export function useViewed(agentId: string, base: DiffBaseMode, files: FileStatus[]) {
  const key = storageKey(agentId, base);
  const [state, setState] = useState(() => ({ key, map: load(key) }));

  useEffect(() => {
    setState((s) => (s.key === key ? s : { key, map: load(key) }));
  }, [key]);

  const update = useCallback(
    (change: (map: ViewedMap) => ViewedMap) =>
      setState((s) => {
        const map = change(s.map);
        localStorage.setItem(s.key, JSON.stringify(map));
        return { key: s.key, map };
      }),
    [],
  );

  const map = state.map;
  return useMemo(
    () => ({
      isViewed: (f: FileStatus) => isViewedIn(map, f),
      markViewed: (paths: string[]) => update((m) => withViewed(m, files, paths)),
      toggle: (f: FileStatus) => update((m) => withToggled(m, f)),
    }),
    [map, files, update],
  );
}
