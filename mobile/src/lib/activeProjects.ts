// Home's project order: the projects the user is working in first, the rest
// as the host listed them. Same rule and the same turn stamps as the desktop
// sidebar's "Active projects first" (src/components/Sidebar/recentProjects.ts),
// so the two apps agree on what is at the top.

import type { Workspace } from "@desktop/api/types/agent";
import { bubbleActive } from "@desktop/components/Sidebar/recentProjects";
import {
  loadProjectActivity,
  type ProjectActivity,
  stampProjectActivity,
} from "@desktop/storage/projectActivity";
import { useEffect, useMemo, useState } from "react";
import { useStore } from "../store";
import { agentsOfProject } from "./agents";
import { useTick } from "./hooks";
import { type Project, projectsOf } from "./projects";

/** Projects with recent activity first, most recent first; the others keep
 *  their incoming order. `turns` is the last turn start per project id. */
export function orderProjects(
  ws: Workspace | null,
  turns: ProjectActivity,
  now: number,
): Project[] {
  const candidates = projectsOf(ws).map((p) => ({
    key: p.id,
    project: p,
    agents: agentsOfProject(ws, p.id),
    runs: [],
  }));
  const { active, rest } = bubbleActive(candidates, now, turns);
  return [...active, ...rest].map((c) => c.project);
}

/** The Home list, ordered. Stamps each turn start on its project and keeps
 *  the stamps locally, the way the desktop sidebar does, so a message sent to
 *  an idle agent counts as activity and survives a relaunch. */
export function useOrderedProjects(): Project[] {
  const workspace = useStore((s) => s.workspace);
  const turnStartedAt = useStore((s) => s.turnStartedAt);
  const activeFirst = useStore((s) => s.activeFirst);
  const [turns, setTurns] = useState(loadProjectActivity);
  useEffect(() => {
    const stamps: ProjectActivity = {};
    for (const a of workspace?.agents ?? []) {
      const t = turnStartedAt[a.id];
      if (t !== undefined && t > (stamps[a.project_id] ?? 0)) stamps[a.project_id] = t;
    }
    // Only a newer start than the one on record is a change; otherwise this
    // would re-stamp (and re-render) on every snapshot.
    setTurns((prev) => {
      const fresh = Object.entries(stamps).some(([k, t]) => t > (prev[k] ?? 0));
      return fresh ? stampProjectActivity(stamps) : prev;
    });
  }, [workspace, turnStartedAt]);
  // Once a minute, so a project that ages past the window while the app is
  // open settles back without waiting for the next snapshot.
  useTick(60_000, activeFirst);
  const now = Date.now();
  return useMemo(
    () => (activeFirst ? orderProjects(workspace, turns, now) : projectsOf(workspace)),
    [workspace, turns, activeFirst, now],
  );
}
