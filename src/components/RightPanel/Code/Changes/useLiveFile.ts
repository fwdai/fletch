import { useEffect, useRef, useState } from "react";
import type { FileStatus } from "@/api";
import { fileRevision } from "@/util/diff";

const size = (f: FileStatus) => f.additions + f.deletions;

/** The file whose revision moved against `prev`, the one whose +/− counts
 *  moved the most when several did — git status has no timestamps, so
 *  magnitude is the best proxy for "most active". Null when nothing moved. */
function mostMoved(files: FileStatus[], prev: Map<string, FileStatus>): string | null {
  let moved: string | null = null;
  let best = -1;
  for (const f of files) {
    const was = prev.get(f.path);
    if (was && fileRevision(was) === fileRevision(f)) continue;
    const delta =
      Math.abs(f.additions - (was?.additions ?? 0)) + Math.abs(f.deletions - (was?.deletions ?? 0));
    if (delta > best) {
      best = delta;
      moved = f.path;
    }
  }
  return moved;
}

/** The file the agent is most likely editing right now, or null while it is
 *  idle. Without an edit stream this approximates: whichever listed file's
 *  revision moved since the last poll. The first poll of a turn has nothing
 *  to compare against, so it guesses the most-changed file until real movement
 *  refines it.
 *
 *  `scope` names the list (agent + diff base). A new scope starts a new
 *  baseline, so switching All/Uncommitted mid-turn doesn't read every newly
 *  listed file as an edit. */
export function useLiveFile(scope: string, files: FileStatus[], busy: boolean): string | null {
  const baseline = useRef<{ scope: string; files: Map<string, FileStatus> } | null>(null);
  const [live, setLive] = useState<string | null>(null);

  useEffect(() => {
    const byPath = new Map(files.map((f) => [f.path, f]));
    const prev = baseline.current?.scope === scope ? baseline.current.files : null;

    // Idle: nothing is live, but keep the baseline current so the next turn's
    // very first edit registers as movement.
    if (!busy) {
      baseline.current = files.length > 0 ? { scope, files: byPath } : null;
      setLive(null);
      return;
    }

    if (!prev) {
      if (files.length === 0) {
        setLive(null);
        return;
      }
      baseline.current = { scope, files: byPath };
      setLive([...files].sort((a, b) => size(b) - size(a))[0].path);
      return;
    }

    baseline.current = { scope, files: byPath };
    const moved = mostMoved(files, prev);
    if (moved) setLive(moved);
  }, [scope, files, busy]);

  return live;
}
