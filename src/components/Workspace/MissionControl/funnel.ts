// MissionControl/funnel.ts — pure logic for routing an inbox issue onto a
// project's roadmap. Routing does not decide anything: the row lands `proposed`
// (a ghost) and the board's existing Accept/Discard is still the ruling, so one
// triage surface serves user ideas, PM proposals, and tracker issues alike.
// Source-agnostic like inbox.ts — it speaks `TrackerIssue`, so GitHub and
// Linear (and a future tracker) flow through the same code. Side-effect-free so
// every rule is unit-tested without the store (funnel.test.ts).

import type { IssueSource, ItemSource, NewRoadmapItem, RoadmapItem, TrackerIssue } from "@/api";

/** Longest body distillation carried into `why`. The `why` is the one line
 *  justifying an item's place on the board, and an issue template pasted whole
 *  would drown the card — the URL on the line above is always there for the
 *  full text. */
const BODY_MAX = 240;

/** Which `ItemSource` a tracker's issues land as. Exhaustive over
 *  `IssueSource`, so adding a tracker fails to compile until the board's glyph
 *  vocabulary knows about it. */
const ITEM_SOURCE: Record<IssueSource, ItemSource> = { github: "github", linear: "linear" };

/** One line of an issue's body: template comments dropped (issue forms are
 *  mostly `<!-- -->` guidance), whitespace collapsed, clipped at a word
 *  boundary. Empty when there is nothing worth carrying. */
export function distillIssueBody(body: string | undefined, maxLen = BODY_MAX): string {
  const text = (body ?? "")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (text.length <= maxLen) return text;
  const clipped = text.slice(0, maxLen);
  const boundary = clipped.lastIndexOf(" ");
  const kept = boundary > maxLen / 2 ? clipped.slice(0, boundary) : clipped;
  return `${kept.replace(/[\s.,;:—-]+$/, "")}…`;
}

/** The `why` a routed issue carries: its URL alone on the first line, the
 *  distilled body under it. That first line is load-bearing, not decoration —
 *  it is the *only* record that this issue was routed (see `routedIssueUrls`),
 *  which is what lets dedup survive a reload without a column of its own. */
export function composeIssueWhy(issue: Pick<TrackerIssue, "url" | "body">): string {
  const body = distillIssueBody(issue.body);
  return body ? `${issue.url}\n${body}` : issue.url;
}

/** The row an inbox issue becomes. `proposed` is the whole point: routing is
 *  triage, not a commitment, so the item arrives as a ghost the user rules on
 *  with the same Accept/Discard every proposal gets. Horizon and rank are left
 *  to the backend's defaults — where it belongs is a planning call, made after
 *  the ruling. */
export function issueToRoadmapItem(issue: TrackerIssue): NewRoadmapItem {
  return {
    title: issue.title,
    why: composeIssueWhy(issue),
    status: "proposed",
    source: ITEM_SOURCE[issue.source],
  };
}

/** Every issue URL already routed onto a board, read off the rows' `why` first
 *  lines. Derived from the rows rather than remembered at the click, so the
 *  inbox still knows after a reload — and so a row discarded on the board
 *  offers its issue again. */
export function routedIssueUrls(rows: readonly Pick<RoadmapItem, "why">[]): Set<string> {
  const urls = new Set<string>();
  for (const row of rows) {
    const first = row.why.split("\n", 1)[0].trim();
    if (/^https?:\/\/\S+$/.test(first)) urls.add(first);
  }
  return urls;
}

/** What an inbox row offers for its issue: `none` when the repo belongs to no
 *  project (there is no board to route onto), `routed` when the issue is
 *  already on one, else the action carrying the project to create in. */
export type FunnelAction =
  | { kind: "none" }
  | { kind: "routed" }
  | { kind: "add"; projectId: string };

/** Decide that action. Project first: without a board there is nothing to say
 *  about the issue at all. */
export function funnelAction(
  projectId: string | undefined,
  issueUrl: string,
  routed: ReadonlySet<string>,
): FunnelAction {
  if (!projectId) return { kind: "none" };
  if (routed.has(issueUrl)) return { kind: "routed" };
  return { kind: "add", projectId };
}
