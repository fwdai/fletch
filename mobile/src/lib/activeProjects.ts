// Home's project order: the projects the user is working in first, the rest
// as the hosts listed them. Same rule and the same turn stamps as the desktop
// sidebar's "Active projects first" (src/components/Sidebar/recentProjects.ts),
// so the two apps agree on what is at the top.

import { bubbleActive } from "@desktop/components/Sidebar/recentProjects";
import {
  loadProjectActivity,
  type ProjectActivity,
  stampProjectActivity,
} from "@desktop/storage/projectActivity";
import { useEffect, useMemo, useState } from "react";
import { type HostState, useApp, useEveryHost, useProjectEntries } from "../store";
import { useTick } from "./hooks";
import type { ProjectEntry } from "./projects";

/** Projects with recent activity on any host first, most recent first; the
 *  others keep their incoming order. `turns` is the last turn start per
 *  project id. */
export function orderProjects(
  entries: readonly ProjectEntry[],
  turns: ProjectActivity,
  now: number,
): ProjectEntry[] {
  const candidates = entries.map((entry) => ({
    key: entry.project.id,
    entry,
    agents: entry.agents.map((h) => h.agent),
    runs: [],
  }));
  const { active, rest } = bubbleActive(candidates, now, turns);
  return [...active, ...rest].map((c) => c.entry);
}

const workspaceOf = (s: HostState) => s.workspace;
const turnsOf = (s: HostState) => s.turnStartedAt;

/** The Home list, every paired host's projects merged and ordered. Stamps each
 *  turn start, on whichever host, on its project and keeps the stamps locally,
 *  the way the desktop sidebar does, so a message sent to an idle agent counts
 *  as activity and survives a relaunch. */
export function useOrderedProjects(): ProjectEntry[] {
  const entries = useProjectEntries();
  const workspaces = useEveryHost(workspaceOf);
  const turnStartedAt = useEveryHost(turnsOf);
  const activeFirst = useApp((s) => s.activeFirst);
  const [turns, setTurns] = useState(loadProjectActivity);
  useEffect(() => {
    const stamps: ProjectActivity = {};
    workspaces.forEach((workspace, i) => {
      for (const a of workspace?.agents ?? []) {
        const t = turnStartedAt[i]?.[a.id];
        if (t !== undefined && t > (stamps[a.project_id] ?? 0)) stamps[a.project_id] = t;
      }
    });
    // Only a newer start than the one on record is a change; otherwise this
    // would re-stamp (and re-render) on every snapshot.
    setTurns((prev) => {
      const fresh = Object.entries(stamps).some(([k, t]) => t > (prev[k] ?? 0));
      return fresh ? stampProjectActivity(stamps) : prev;
    });
  }, [workspaces, turnStartedAt]);
  // Once a minute, so a project that ages past the window while the app is
  // open settles back without waiting for the next snapshot.
  useTick(60_000, activeFirst);
  const now = Date.now();
  return useMemo(
    () => (activeFirst ? orderProjects(entries, turns, now) : entries),
    [entries, turns, activeFirst, now],
  );
}
