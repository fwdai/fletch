import { useEffect, useMemo, useRef } from "react";
import type { DiffHunk } from "@/util/diff";
import { type FileDiffState, freshKey } from "../DiffView";

const addedKeys = (hunks: DiffHunk[]) => {
  const keys = new Set<string>();
  for (const h of hunks) for (const l of h.lines) if (l.op === "add") keys.add(freshKey(l));
  return keys;
};

/** The added lines of `diff` that were not there the last time this same diff
 *  (`id`: file + base) was shown — what the agent wrote since. Opening a diff,
 *  or switching scope, flashes nothing: there is no earlier version to compare. */
export function useFreshLines(id: string, diff: FileDiffState): Set<string> {
  const seen = useRef<{ id: string; keys: Set<string> } | null>(null);
  const { hunks, loaded } = diff;

  const fresh = useMemo(() => {
    const prev = seen.current;
    if (!loaded || prev?.id !== id) return new Set<string>();
    return new Set([...addedKeys(hunks)].filter((k) => !prev.keys.has(k)));
  }, [id, hunks, loaded]);

  useEffect(() => {
    if (loaded) seen.current = { id, keys: addedKeys(hunks) };
  }, [id, hunks, loaded]);

  return fresh;
}
