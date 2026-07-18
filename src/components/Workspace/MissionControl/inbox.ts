// MissionControl/inbox.ts — pure logic for the Home issue inbox: the
// branch-name suggestion, the "Start work" brief composition, and merging
// per-repo issue lists into one ordered set of rows. Source-agnostic — it
// speaks `TrackerIssue`, so GitHub issues and Linear tickets (and future
// sources) flow through the same code. Kept side-effect-free so each piece
// is unit-tested without the network or the store (inbox.test.ts).

import { issueDisplayKey, type TrackerIssue, type TrackerLabel } from "@/api";

/** A tracked repo's fetched issues, tagged with its display label. */
export interface InboxRepo {
  repoPath: string;
  /** Human label for the repo (project/repo name), shown when >1 repo. */
  repoLabel: string;
  issues: TrackerIssue[];
}

/** One rendered inbox row: an issue plus the repo it belongs to. Keyed by
 *  repo+source+key so the same issue number in two repos (or a GitHub #12
 *  next to a Linear ENG-12) never collides. */
export interface InboxRow {
  key: string;
  repoPath: string;
  repoLabel: string;
  issue: TrackerIssue;
}

/** Slugify an issue title for a branch name: lowercase, non-alphanumerics to
 *  single dashes, trimmed, and clamped to a handful of words so the branch
 *  stays short and conventional. Empty when the title has no usable words. */
export function slugifyTitle(title: string, maxWords = 5, maxLen = 40): string {
  const words = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, maxWords);
  return words.join("-").slice(0, maxLen).replace(/-+$/, "");
}

/** Conventional branch prefix inferred from the issue's labels — `feat` for a
 *  feature/enhancement, `chore` for chore/docs/deps, else `fix` (the safe
 *  default for a bug or an unlabeled issue). */
export function branchKind(labels: TrackerLabel[]): "fix" | "feat" | "chore" {
  const names = labels.map((l) => l.name.toLowerCase());
  const has = (...needles: string[]) =>
    names.some((n) => needles.some((needle) => n.includes(needle)));
  if (has("feature", "enhancement", "feat")) return "feat";
  if (has("chore", "docs", "documentation", "dependenc")) return "chore";
  return "fix";
}

/** Suggested branch name for an issue, e.g. `fix/123-login-crash` (GitHub) or
 *  `fix/eng-123-login-crash` (Linear). Visible and editable in the composed
 *  brief — never hidden magic. Falls back to just the key when the title
 *  yields no slug. */
export function suggestBranchName(issue: Pick<TrackerIssue, "key" | "title" | "labels">): string {
  const kind = branchKind(issue.labels);
  const ref = issue.key.toLowerCase();
  const slug = slugifyTitle(issue.title);
  return slug ? `${kind}/${ref}-${slug}` : `${kind}/${ref}`;
}

/** Human name of an issue's source, for the brief's opening line. */
const SOURCE_LABEL: Record<TrackerIssue["source"], string> = {
  github: "GitHub",
  linear: "Linear",
};

/** Compose the new-task brief for "Start work" / the composer's issue picker:
 *  issue reference, title, body, and a visible/editable branch suggestion.
 *  The closing trailer (`Closes #N` / `Fixes ENG-N`) is added reliably by the
 *  backend at PR time, so the brief needn't instruct it. */
export function composeIssueBrief(issue: TrackerIssue): string {
  const branch = suggestBranchName(issue);
  const parts = [`${SOURCE_LABEL[issue.source]} issue ${issueDisplayKey(issue)}: ${issue.title}`];
  const body = issue.body?.trim();
  if (body) parts.push(body);
  parts.push(issue.url);
  parts.push(`When you open the PR, use the branch name \`${branch}\`.`);
  return parts.join("\n\n");
}

/** Merge per-repo issue lists into one ordered set of rows, newest-updated
 *  first (issues with no timestamp sort last), capped at `limit`. */
export function deriveInboxRows(repos: InboxRepo[], limit = 20): InboxRow[] {
  const rows: InboxRow[] = [];
  for (const repo of repos) {
    for (const issue of repo.issues) {
      rows.push({
        key: `${repo.repoPath}#${issue.source}:${issue.key}`,
        repoPath: repo.repoPath,
        repoLabel: repo.repoLabel,
        issue,
      });
    }
  }
  rows.sort((a, b) => (b.issue.updated_at ?? 0) - (a.issue.updated_at ?? 0));
  return rows.slice(0, limit);
}
