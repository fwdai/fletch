// MissionControl/useIssueFunnel.ts — the inbox's roadmap side: which issues are
// already on a board, which ones the user has turned down, and the one call that
// routes a new one there. Creation goes through the typed `roadmap_create_item`
// command, which records the item's `created` history line in the same
// transaction — the funnel adds no writer of its own. The rules it applies are
// pure (funnel.ts).
//
// The boards are *followed*, not polled. This used to re-read
// `roadmap_list_items` for every project every two minutes, which bought a
// two-minute window in which the inbox and the board disagreed: discarding a
// routed ghost on the board left this section saying "On roadmap", and the
// click-dedup could only see ghosts this window had created. It was the last
// roadmap consumer beside the event spine; it now rides the same
// `roadmap:item` / `roadmap:item-deleted` stream the board does (see
// src/roadmapRows.ts), so "is it already there?" is answered by state that
// changes when the board does. No safety poll: a subscription that has to be
// re-checked on a timer is a subscription nobody trusts, and the board itself
// has never needed one.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type RoadmapItem, type TrackerIssue } from "@/api";
import { useRoadmapRows } from "@/roadmapRows";
import { getProjectSettings } from "@/storage/projectSettings";
import {
  DECLINED_ISSUES_KEY,
  type FunnelAction,
  funnelAction,
  issueToRoadmapItem,
  parseDeclinedIssues,
  routedIssueUrls,
} from "./funnel";

export interface IssueFunnel {
  /** What this issue's row should offer, given the project its repo belongs to
   *  (empty/undefined when it belongs to none). Nothing, until that project's
   *  board has been read. */
  actionFor: (projectId: string | undefined, issue: TrackerIssue) => FunnelAction;
  /** Route the issue onto the project's board as a ghost row awaiting a ruling. */
  route: (projectId: string, issue: TrackerIssue) => Promise<void>;
}

export function useIssueFunnel(projectIds: string[]): IssueFunnel {
  /** Issues declined on each board, as stored (`project_settings`). Read once per
   *  project rather than followed: the tombstone is written by the same delete
   *  that emits `roadmap:item-deleted`, so the live half below covers every
   *  refusal made while this section is mounted, and this covers the ones made
   *  before it was. */
  const [stored, setStored] = useState<Record<string, ReadonlySet<string>>>({});
  /** Refusals seen live — a routed ghost that was just deleted. Held separately
   *  from `stored` so a re-read can never drop one. */
  const [seen, setSeen] = useState<Record<string, ReadonlySet<string>>>({});
  // In-flight issue urls, in a ref rather than state: two fast clicks land in
  // the same render, and a second ghost row is not something a reload undoes.
  const inFlight = useRef(new Set<string>());

  const declineSeen = useCallback((projectId: string, url: string) => {
    setSeen((prev) => {
      const next = new Set(prev[projectId] ?? []);
      if (next.has(url)) return prev;
      next.add(url);
      return { ...prev, [projectId]: next };
    });
  }, []);

  /** The rows as of the last render, for the delete listener — which needs the row
   *  that is *going away*, and by the time a re-render carries the new list that
   *  row is exactly what is missing from it. */
  const rowsForDelete = useRef<ReadonlyMap<string, RoadmapItem>>(new Map());
  const onDeleted = useCallback(
    (id: string) => {
      const row = rowsForDelete.current.get(id);
      // Only a *ghost* routed from a tracker is a refusal: deleting an item the
      // user already accepted is a different decision, and that issue is
      // legitimately offered again. Keyed on `issue_url` alone (not the legacy
      // `why` first line) so this agrees exactly with what the backend's delete
      // path records — a pre-0036 row leaves no tombstone either way.
      if (!row || row.status !== "proposed" || !row.issue_url) return;
      declineSeen(row.project_id, row.issue_url);
    },
    [declineSeen],
  );

  const { rows, load } = useRoadmapRows(projectIds, { onDeleted });
  rowsForDelete.current = useMemo(() => new Map(rows.map((r) => [r.id, r])), [rows]);

  const key = projectIds.join(" ");
  useEffect(() => {
    const ids = key ? key.split(" ") : [];
    if (ids.length === 0) {
      setStored({});
      return;
    }
    let alive = true;
    void (async () => {
      const entries = await Promise.all(
        ids.map(async (id) => {
          const all = await getProjectSettings(id).catch(() => ({}) as Record<string, string>);
          return [id, parseDeclinedIssues(all[DECLINED_ISSUES_KEY])] as const;
        }),
      );
      if (alive) setStored(Object.fromEntries(entries));
    })();
    return () => {
      alive = false;
    };
  }, [key]);

  // Per project, never pooled: the same origin repo pinned at two paths in two
  // projects would otherwise read "on roadmap" on both boards after one routing.
  const routedByProject = useMemo(() => {
    const byProject = new Map<string, RoadmapItem[]>();
    for (const row of rows) {
      const group = byProject.get(row.project_id);
      if (group) group.push(row);
      else byProject.set(row.project_id, [row]);
    }
    const out = new Map<string, ReadonlySet<string>>();
    for (const [id, group] of byProject) out.set(id, routedIssueUrls(group));
    return out;
  }, [rows]);

  /** Both halves together: what was stored before this section mounted, and what
   *  it has watched happen since. */
  const declinedByProject = useMemo(() => {
    const out = new Map<string, Set<string>>();
    for (const [id, urls] of Object.entries(stored)) out.set(id, new Set(urls));
    for (const [id, urls] of Object.entries(seen)) {
      const set = out.get(id) ?? new Set<string>();
      for (const url of urls) set.add(url);
      out.set(id, set);
    }
    return out;
  }, [stored, seen]);

  const actionFor = useCallback(
    (projectId: string | undefined, issue: TrackerIssue) => {
      // Unknown board (unread or failed) → no roadmap action, the same quiet
      // degradation the section gives a tracker that won't answer. Offering Add
      // here is what creates duplicates: a failed read is not an empty board.
      if (!projectId || load.get(projectId) !== "loaded") return { kind: "none" } as const;
      return funnelAction(
        projectId,
        issue.url,
        routedByProject.get(projectId) ?? new Set(),
        declinedByProject.get(projectId) ?? new Set(),
      );
    },
    [load, routedByProject, declinedByProject],
  );

  const route = useCallback(async (projectId: string, issue: TrackerIssue) => {
    if (inFlight.current.has(issue.url)) return;
    inFlight.current.add(issue.url);
    try {
      // Nothing to fold in: the create emits `roadmap:item`, which the row
      // subscription applies — the same path a ghost the PM proposed takes. The
      // old optimistic fold-in existed only because the poll would otherwise take
      // two minutes to agree, and it was the poll's own clobber window.
      await api.roadmapCreateItem(projectId, issueToRoadmapItem(issue));
    } finally {
      inFlight.current.delete(issue.url);
    }
  }, []);

  return { actionFor, route };
}
