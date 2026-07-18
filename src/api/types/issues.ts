// Generalized issue-tracker types — the frontend mirror of Rust's
// `issues::TrackerIssue`. The Home inbox, the composer's issue picker, and
// the "Start work" brief all consume this one shape, so adding a source
// (Asana, Trello, …) never touches the UI plumbing.

/** Which tracker an issue came from. */
export type IssueSource = "github" | "linear";

/** One label on an issue. `color` is a 6-hex assignment with no leading `#`
 *  (GitHub's native form; other sources are normalized to it). */
export interface TrackerLabel {
  name: string;
  color?: string;
}

/** An open issue from any connected tracker. `key` is the canonical
 *  reference persisted as a draft's `issueRef` and consumed by the PR
 *  closing trailer: `"123"` for GitHub, `"ENG-123"` for Linear. */
export interface TrackerIssue {
  source: IssueSource;
  key: string;
  title: string;
  url: string;
  labels: TrackerLabel[];
  assignee?: string;
  /** `updatedAt` as ms-epoch, for ordering and the "updated N ago" hint. */
  updated_at?: number;
  body?: string;
}

/** The human-facing form of an issue's key: GitHub numbers read as `#123`,
 *  tracker keys (`ENG-123`) as themselves. */
export function issueDisplayKey(issue: Pick<TrackerIssue, "source" | "key">): string {
  return issue.source === "github" ? `#${issue.key}` : issue.key;
}

/** Linear connection state — `authenticated` gates Linear affordances. */
export interface LinearStatus {
  authenticated: boolean;
  user: string | null;
}

/** One Linear team, for the per-project team picker. */
export interface LinearTeam {
  id: string;
  key: string;
  name: string;
}
