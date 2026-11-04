// issueBrief.ts — pure logic for starting work from a tracker issue: the
// branch-name suggestion and the brief the composer's issue picker seeds the
// prompt with. Source-agnostic — it speaks `TrackerIssue`, so GitHub issues and
// Linear tickets (and future sources) flow through the same code. Kept
// side-effect-free so each piece is unit-tested without the network or the
// store (issueBrief.test.ts).

import { type IssueComment, issueDisplayKey, type TrackerIssue, type TrackerLabel } from "@/api";

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

/** Compose the new-task brief for the composer's issue picker: issue
 *  reference, title, body, the issue's discussion (when fetched), and a
 *  visible/editable branch suggestion. The closing trailer (`Closes #N` /
 *  `Fixes ENG-N`) is added reliably by the backend at PR time, so the brief
 *  needn't instruct it. Comments arrive oldest-first and already capped by
 *  the backend; here they only get laid out. */
export function composeIssueBrief(issue: TrackerIssue, comments: IssueComment[] = []): string {
  const branch = suggestBranchName(issue);
  const parts = [`${SOURCE_LABEL[issue.source]} issue ${issueDisplayKey(issue)}: ${issue.title}`];
  const body = issue.body?.trim();
  if (body) parts.push(body);
  if (comments.length > 0) {
    const thread = comments.map((c) => `${c.author ?? "someone"}: ${c.body.trim()}`).join("\n\n");
    parts.push(`Discussion on the issue (oldest first):\n\n${thread}`);
  }
  parts.push(issue.url);
  parts.push(`When you open the PR, use the branch name \`${branch}\`.`);
  return parts.join("\n\n");
}
