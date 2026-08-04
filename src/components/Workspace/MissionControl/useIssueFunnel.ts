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
   *  (empty/undefined when it belongs to none). Nothing, until that project's
   *  board has been read. */
  actionFor: (projectId: string | undefined, issue: TrackerIssue) => FunnelAction;
  /** Route the issue onto the project's board as a ghost row awaiting a ruling. */
  route: (projectId: string, issue: TrackerIssue) => Promise<void>;
}

/** A project's board as the funnel knows it. A read that failed is *not* an
 *  empty board: the backend dedups nothing, so treating "we don't know" as
 *  "nothing is routed" would re-enable Add on every already-routed row and let
 *  one click stack a second ghost. A project absent from the record has not been
 *  read yet, which is the same unknown. */
type Board = { status: "loaded"; rows: RoadmapItem[] } | { status: "failed" };

export function useIssueFunnel(projectIds: string[]): IssueFunnel {
  const [boards, setBoards] = useState<Record<string, Board>>({});
  // In-flight issue urls, in a ref rather than state: two fast clicks land in
  // the same render, and a second ghost row is not something a reload undoes.
  const inFlight = useRef(new Set<string>());

  const poll = useCallback(async () => {
    if (projectIds.length === 0) return;
    const entries = await Promise.all(
      projectIds.map(async (id) => {
        try {
          return [id, { status: "loaded", rows: await api.roadmapListItems(id) }] as const;
        } catch {
          return [id, { status: "failed" }] as const;
        }
      }),
    );
    setBoards(Object.fromEntries(entries));
  }, [projectIds]);

  usePoll(poll, POLL_MS, [poll]);

  // Per project, never pooled: the same origin repo pinned at two paths in two
  // projects would otherwise read "on roadmap" on both boards after one routing.
  const routedByProject = useMemo(() => {
    const byProject: Record<string, Set<string>> = {};
    for (const [id, board] of Object.entries(boards)) {
      if (board.status === "loaded") byProject[id] = routedIssueUrls(board.rows);
    }
    return byProject;
  }, [boards]);

  const actionFor = useCallback(
    (projectId: string | undefined, issue: TrackerIssue) => {
      const routed = projectId ? routedByProject[projectId] : undefined;
      // Unknown board (unread or failed) → no roadmap action, the same quiet
      // degradation the section gives a tracker that won't answer. Offering Add
      // here is what creates duplicates.
      if (!routed) return { kind: "none" } as const;
      return funnelAction(projectId, issue.url, routed);
    },
    [routedByProject],
  );

  const route = useCallback(async (projectId: string, issue: TrackerIssue) => {
    if (inFlight.current.has(issue.url)) return;
    inFlight.current.add(issue.url);
    try {
      const row = await api.roadmapCreateItem(projectId, issueToRoadmapItem(issue));
      // Fold the stored row in rather than remembering the click: "on roadmap"
      // stays a function of board rows, so this agrees with the next poll and
      // with a reload. Only into a board we have actually read — inventing one
      // from a single row would claim the rest of that board is empty.
      setBoards((prev) => {
        const board = prev[projectId];
        if (board?.status !== "loaded") return prev;
        return { ...prev, [projectId]: { status: "loaded", rows: [...board.rows, row] } };
      });
    } finally {
      inFlight.current.delete(issue.url);
    }
  }, []);

  return { actionFor, route };
}
