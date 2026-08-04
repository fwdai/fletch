// MissionControl/useIssueFunnel.ts — the inbox's roadmap side: which issues are
// already on a board, and the one call that routes a new one there. Creation
// goes through the typed `roadmap_create_item` command, which records the
// item's `created` history line in the same transaction — the funnel adds no
// writer of its own. The rules it applies are pure (funnel.ts).

import { useCallback, useMemo, useRef, useState } from "react";
import { api, type RoadmapItem, type TrackerIssue } from "@/api";
import { usePoll } from "@/util/hooks";
import { type FunnelAction, funnelAction, issueToRoadmapItem, routedIssueUrls } from "./funnel";

/** Same slow cadence as the inbox itself: this only answers "is it already on
 *  the board?", and a routed row is folded in at the click, so the poll is just
 *  what catches a board changed on another surface. */
const POLL_MS = 120_000;

export interface IssueFunnel {
  /** What this issue's row should offer, given the project its repo belongs to
   *  (empty/undefined when it belongs to none). */
  actionFor: (projectId: string | undefined, issue: TrackerIssue) => FunnelAction;
  /** Route the issue onto the project's board as a ghost row awaiting a ruling. */
  route: (projectId: string, issue: TrackerIssue) => Promise<void>;
}

export function useIssueFunnel(projectIds: string[]): IssueFunnel {
  const [rowsByProject, setRowsByProject] = useState<Record<string, RoadmapItem[]>>({});
  // In-flight issue urls, in a ref rather than state: two fast clicks land in
  // the same render, and a second ghost row is not something a reload undoes.
  const inFlight = useRef(new Set<string>());

  const poll = useCallback(async () => {
    if (projectIds.length === 0) return;
    const entries = await Promise.all(
      projectIds.map(async (id) => [id, await api.roadmapListItems(id).catch(() => [])] as const),
    );
    setRowsByProject(Object.fromEntries(entries));
  }, [projectIds]);

  usePoll(poll, POLL_MS, [poll]);

  const routed = useMemo(
    () => routedIssueUrls(Object.values(rowsByProject).flat()),
    [rowsByProject],
  );

  const actionFor = useCallback(
    (projectId: string | undefined, issue: TrackerIssue) =>
      funnelAction(projectId, issue.url, routed),
    [routed],
  );

  const route = useCallback(async (projectId: string, issue: TrackerIssue) => {
    if (inFlight.current.has(issue.url)) return;
    inFlight.current.add(issue.url);
    try {
      const row = await api.roadmapCreateItem(projectId, issueToRoadmapItem(issue));
      // Fold the stored row in rather than remembering the click: "on roadmap"
      // stays a function of board rows, so this agrees with the next poll and
      // with a reload.
      setRowsByProject((prev) => ({ ...prev, [projectId]: [...(prev[projectId] ?? []), row] }));
    } finally {
      inFlight.current.delete(issue.url);
    }
  }, []);

  return { actionFor, route };
}
