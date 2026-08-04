// MissionControl/funnel.ts — pure logic for routing an inbox issue onto a
// project's roadmap. Routing does not decide anything: the row lands `proposed`
// (a ghost) and the board's existing Accept/Discard is still the ruling, so one
// triage surface serves user ideas, PM proposals, and tracker issues alike.
// Source-agnostic like inbox.ts — it speaks `TrackerIssue`, so GitHub and
// Linear (and a future tracker) flow through the same code. Side-effect-free so
// every rule is unit-tested without the store (funnel.test.ts).

import type { IssueSource, ItemSource, NewRoadmapItem, RoadmapItem, TrackerIssue } from "@/api";

/** `project_settings` key holding the issues the user has turned down, as a JSON
 *  array of URLs — written by the backend's delete path when it removes a
 *  `proposed` row carrying an `issue_url` (roadmap/store.rs `decline_issue`).
 *
 *  A discard deletes the row, so the refusal has nowhere on the board to live;
 *  without this the inbox re-offered a discarded issue on every read, forever. */
export const DECLINED_ISSUES_KEY = "roadmap.declined_issues";

/** Read that stored list. A missing, blank or unparseable value is an empty set:
 *  a corrupt setting must cost the dedup, never the inbox. */
export function parseDeclinedIssues(value: string | undefined): Set<string> {
  if (!value) return new Set();
  try {
    const parsed: unknown = JSON.parse(value);
    return new Set(
      Array.isArray(parsed) ? parsed.filter((u): u is string => typeof u === "string") : [],
    );
  } catch {
    return new Set();
  }
}

/** Longest body distillation carried into `why`. The `why` is the one line
 *  justifying an item's place on the board, and an issue template pasted whole
 *  would drown the card — the URL on the line above is always there for the
 *  full text. */
const BODY_MAX = 240;

/** Which `ItemSource` a tracker's issues land as. Exhaustive over
 *  `IssueSource`, so adding a tracker fails to compile until the board's glyph
 *  vocabulary knows about it. */
const ITEM_SOURCE: Record<IssueSource, ItemSource> = { github: "github", linear: "linear" };

/** One line of an issue's body: markdown that carries no prose dropped —
 *  template comments (issue forms are mostly `<!-- -->` guidance), fenced code,
 *  screenshots — links unwrapped to their text, whitespace collapsed, clipped at
 *  a word boundary. The clip budget is small, so noise spent on it is prose
 *  lost: a body opening with a screenshot and a stack trace would otherwise
 *  distill to a `why` that says nothing. Empty when there is nothing worth
 *  carrying. */
export function distillIssueBody(body: string | undefined, maxLen = BODY_MAX): string {
  const text = (body ?? "")
    .replace(/<!--[\s\S]*?-->/g, " ")
    // Unterminated fence (a paste cut short) swallows the rest, which is the
    // code it opened.
    .replace(/```[\s\S]*?(?:```|$)/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (text.length <= maxLen) return text;
  const clipped = text.slice(0, maxLen);
  const boundary = clipped.lastIndexOf(" ");
  const kept = boundary > maxLen / 2 ? clipped.slice(0, boundary) : clipped;
  return `${kept.replace(/[\s.,;:—-]+$/, "")}…`;
}

/** The `why` a routed issue carries: its URL alone on the first line, the
 *  distilled body under it.
 *
 *  The URL used to be load-bearing here — it was the *only* record that the issue
 *  had been routed, which is what let dedup survive a reload without a column.
 *  Since migration 0036 the record is `issue_url` on the row, and this line is
 *  what it always looked like: provenance the reader can click. Dedup no longer
 *  depends on it, so editing the rationale (or accepting a PM proposal that
 *  rewrites it) can't re-offer an issue that is already on the board. */
export function composeIssueWhy(issue: Pick<TrackerIssue, "url" | "body">): string {
  const body = distillIssueBody(issue.body);
  return body ? `${issue.url}\n${body}` : issue.url;
}

/** The row an inbox issue becomes. `proposed` is the whole point: routing is
 *  triage, not a commitment, so the item arrives as a ghost the user rules on
 *  with the same Accept/Discard every proposal gets. Horizon and rank are left
 *  to the backend's defaults — where it belongs is a planning call, made after
 *  the ruling.
 *
 *  `issue_url` is the durable routing record; the backend accepts it only at
 *  creation, so this call is the one place it is ever written. */
export function issueToRoadmapItem(issue: TrackerIssue): NewRoadmapItem {
  return {
    title: issue.title,
    why: composeIssueWhy(issue),
    status: "proposed",
    source: ITEM_SOURCE[issue.source],
    issue_url: issue.url,
  };
}

/** Which issue a row was routed from, or null.
 *
 *  `issue_url` first, because that is the column the funnel writes and nothing
 *  edits. The `why` first line is the legacy reader, kept for rows created before
 *  migration 0036: their URL only ever lived there, and 0036 deliberately does
 *  not backfill (parsing user-editable prose into a column that claims to be
 *  canonical would re-introduce the very thing the column fixes). New rows never
 *  reach the fallback. */
export function routedIssueUrl(row: Pick<RoadmapItem, "why" | "issue_url">): string | null {
  if (row.issue_url) return row.issue_url;
  const first = row.why.split("\n", 1)[0].trim();
  return /^https?:\/\/\S+$/.test(first) ? first : null;
}

/** Every issue URL already routed onto a board. Derived from the rows rather than
 *  remembered at the click, so the inbox agrees with the board after a reload —
 *  and so an issue whose item was *accepted* and later shipped still reads as
 *  routed while the row lives. */
export function routedIssueUrls(
  rows: readonly Pick<RoadmapItem, "why" | "issue_url">[],
): Set<string> {
  const urls = new Set<string>();
  for (const row of rows) {
    const url = routedIssueUrl(row);
    if (url) urls.add(url);
  }
  return urls;
}

/** What an inbox row offers for its issue: `none` when there is no board to
 *  route onto or no way to tell the issue apart later, `routed` when the issue
 *  is already on one, `declined` when the user has already turned it down, else
 *  the action carrying the project to create in. */
export type FunnelAction =
  | { kind: "none" }
  | { kind: "routed" }
  | { kind: "declined" }
  | { kind: "add"; projectId: string };

/** Decide that action, against the routed and declined urls of *that project's*
 *  board — the same origin repo can be pinned in two projects, and routing (or
 *  refusing) an issue in one says nothing about the other. Project first: without
 *  a board there is nothing to say about the issue at all.
 *
 *  `routed` outranks `declined`: an issue that was discarded and then routed
 *  again is on the board now, and the board is the newer fact. (The tombstone is
 *  never cleared, deliberately — the list is small, append-only, and re-declining
 *  is idempotent.) */
export function funnelAction(
  projectId: string | undefined,
  issueUrl: string,
  routed: ReadonlySet<string>,
  declined: ReadonlySet<string> = new Set(),
): FunnelAction {
  if (!projectId) return { kind: "none" };
  // No url is no dedup key: an urlless issue would offer `add` forever and stack
  // a fresh ghost on every click. Offering nothing is the honest answer.
  if (!issueUrl) return { kind: "none" };
  if (routed.has(issueUrl)) return { kind: "routed" };
  if (declined.has(issueUrl)) return { kind: "declined" };
  return { kind: "add", projectId };
}
