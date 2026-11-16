// What the add menu's side lists show, as plain data: each tracker's, repo's
// and board's answer turned into the rows of a `RefList`. Pure, so the
// ordering and the filtering are tested without a menu (entries.test.ts).

import {
  type Horizon,
  type IssueSource,
  issueDisplayKey,
  type PrSummary,
  type RoadmapItem,
  type TrackerIssue,
} from "@/api";
import type { RefEntry } from "./RefList";

const SOURCE_LABEL: Record<IssueSource, string> = { github: "GitHub", linear: "Linear" };
const HORIZON_LABEL: Record<Horizon, string> = { now: "Now", next: "Next", later: "Later" };
const HORIZON_ORDER: Horizon[] = ["now", "next", "later"];

export function issueId(issue: Pick<TrackerIssue, "source" | "key">): string {
  return `${issue.source}:${issue.key}`;
}

/** Open issues by tracker, so each heading shows once. The sort is stable:
 *  each tracker keeps the order it answered in. */
export function issueEntries(issues: TrackerIssue[]): RefEntry[] {
  return [...issues]
    .sort((a, b) => a.source.localeCompare(b.source))
    .map((issue) => ({
      id: issueId(issue),
      tag: issueDisplayKey(issue),
      title: issue.title,
      group: SOURCE_LABEL[issue.source],
    }));
}

/** Open pull requests, newest first. */
export function prEntries(prs: PrSummary[]): RefEntry[] {
  return [...prs]
    .sort((a, b) => b.number - a.number)
    .map((pr) => ({ id: String(pr.number), tag: `#${pr.number}`, title: pr.title }));
}

/** A roadmap item an agent can be pointed at by hand: agreed to (`open`) and
 *  not yet anyone's. The rule the board's "Send to an agent" goes by — a
 *  proposal isn't agreed work, and from `queued` on the queue owns the row. */
export function isPickable(item: RoadmapItem): boolean {
  return item.status === "open" && !item.agent_id;
}

/** The roadmap items there are to pick up, under their horizon, in board
 *  order within each (the list arrives ranked, and the sort is stable). */
export function roadmapEntries(items: RoadmapItem[]): RefEntry[] {
  return items
    .filter(isPickable)
    .sort((a, b) => HORIZON_ORDER.indexOf(a.horizon) - HORIZON_ORDER.indexOf(b.horizon))
    .map((item) => ({
      id: item.id,
      tag: item.code,
      title: item.title,
      group: HORIZON_LABEL[item.horizon],
    }));
}
