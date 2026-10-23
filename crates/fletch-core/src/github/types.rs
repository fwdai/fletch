//! Public types shared across the GitHub operations — the IPC surface,
//! unchanged from the gh module.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

impl PrStatus {
    /// Stable lowercase form, matching the serde serialization. Used as the
    /// on-disk value in `worktrees.pr_state`.
    pub fn as_str(&self) -> &'static str {
        match self {
            PrStatus::Open => "open",
            PrStatus::Merged => "merged",
            PrStatus::Closed => "closed",
        }
    }

    /// Inverse of [`as_str`](Self::as_str), for rows written by this app.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(PrStatus::Open),
            "merged" => Some(PrStatus::Merged),
            "closed" => Some(PrStatus::Closed),
            _ => None,
        }
    }
}

/// GitHub's coarse `mergeable` verdict — the *only* merge signal when the
/// richer `MergeState` (from `mergeStateStatus`) is unavailable. Deliberately
/// tri-state: GitHub computes mergeability lazily, so `Unknown` ("not computed
/// yet", normal for a while after any push) must stay distinct from
/// `Conflicting` (a real conflict) — collapsing both to a bool made the panel
/// claim "can't merge — update your branch" for perfectly mergeable PRs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeableState {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrState {
    pub number: u32,
    pub url: String,
    pub state: PrStatus,
    pub title: String,
    pub mergeable: MergeableState,
    /// GitHub's createdAt / mergedAt as ms-epoch, when reported. Stamped onto
    /// `worktrees.pr_opened_at/pr_merged_at` by every PR-state fetch path so
    /// per-day PR history accrues locally (see `record_pr_times`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<i64>,
    /// The PR's head branch. A checkout can hold several PRs, each from its
    /// own branch, so the checkout's current branch doesn't name it. `None`
    /// when the source didn't report it (an older snapshot row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// Everything an agent asks about a pull request, flat, from one REST read of
/// the PR object. The fields are the ones agents pulled through `gh pr view
/// --json` before the op existed: base, state, mergeability, head and its sha,
/// title, body, merge gate, draft flag, sizes. `pr_view`'s answer.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PrDetail {
    pub number: u32,
    pub url: String,
    pub state: PrStatus,
    pub draft: bool,
    pub title: String,
    pub body: String,
    /// Head branch name.
    pub branch: String,
    pub head_sha: String,
    pub base: String,
    pub mergeable: MergeableState,
    /// GitHub's combined merge gate: what actually decides whether the PR can
    /// land (`clean`, `blocked`, `behind`, `dirty`, …).
    pub merge_state: MergeState,
    pub author: String,
    pub commits: u32,
    pub changed_files: u32,
    pub additions: u32,
    pub deletions: u32,
    /// ISO-8601 as GitHub gives them.
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
}

/// One row of `pr_list`: enough to find the PR for a branch, walk a stack
/// (`branch` → `base`) or see what merged lately, without a second read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PrListEntry {
    pub number: u32,
    pub title: String,
    pub state: PrStatus,
    pub draft: bool,
    /// Head branch name.
    pub branch: String,
    pub base: String,
    pub url: String,
    pub author: String,
    /// ISO-8601 as GitHub gives it.
    pub updated_at: String,
}

/// What `pr_list` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrListFilter {
    /// "open" | "closed" | "all" — GitHub's own vocabulary; a merged PR is
    /// a closed one whose row says `merged`.
    pub state: String,
    /// Head branch, in this repository.
    pub head: Option<String>,
    pub base: Option<String>,
    pub limit: u32,
}

/// Lightweight PR summary for the composer's "#" mention autocomplete —
/// just enough to list and reference a PR by number.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrSummary {
    pub number: u32,
    pub title: String,
    pub state: PrStatus,
}

/// One label on an issue, for the Home inbox's quiet chips. `color` is
/// GitHub's 6-hex assignment (no leading `#`), used subtly when present.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IssueLabel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// An open GitHub issue for the Home inbox — enough to list it, show its
/// labels/assignee, and seed a "Start work" brief (title + body + url).
#[derive(Debug, Clone, serde::Serialize)]
pub struct IssueSummary {
    pub number: u32,
    pub title: String,
    pub url: String,
    pub labels: Vec<IssueLabel>,
    /// Assignee login when the issue is assigned to someone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// `updatedAt` as ms-epoch, for the "updated N ago" hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// Issue body, carried so "Start work" composes the brief without a
    /// second round-trip. `None`/empty when the issue has no description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// One comment on an issue, as the agent's `issue_view` returns it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IssueCommentRow {
    /// Author's login; empty for a deleted account.
    pub author: String,
    pub body: String,
    /// ISO-8601 as GitHub gives it.
    pub created_at: String,
}

/// An issue, whole, for an agent asked to work on or triage it. Also the row
/// shape of `issue_list`, where `comments` is left empty.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IssueDetail {
    pub number: u32,
    pub title: String,
    /// "open" | "closed"
    pub state: String,
    pub url: String,
    pub author: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
    pub comments_count: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<IssueCommentRow>,
}

/// GitHub's combined merge gate (`mergeStateStatus`), normalized. This — not
/// `mergeable` — is what actually decides whether a PR can land (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeState {
    Clean,
    Blocked,
    Unstable,
    Behind,
    Dirty,
    Draft,
    HasHooks,
    Unknown,
}

/// One CI check, normalized from the `statusCheckRollup` contexts (which mix
/// `CheckRun` and legacy `StatusContext` shapes).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckRun {
    /// GitHub's check-run id — for a GitHub Actions check, the job id its
    /// log and annotations are read by (`pr_check_log`). `None` for a legacy
    /// commit status, which has no run behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub name: String,
    /// "queued" | "in_progress" | "completed"
    pub status: String,
    /// "success" | "failure" | "neutral" | "cancelled" | "skipped" |
    /// "timed_out" | "action_required" | "stale" — None until completed.
    pub conclusion: Option<String>,
    /// Branch-protection data needs an extra (often unauthorized) API call,
    /// so this is always `false` for now — the merge gate comes from
    /// `merge_state` instead (spec §6 fallback).
    pub required: bool,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Rich PR merge-gate + per-check detail (spec §6). Heavier than `pr_view`
/// — callers poll it on a slow cadence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrChecks {
    pub merge_state: MergeState,
    /// "none" | "pending" | "passing" | "failing" — checks-only summary.
    pub rollup: String,
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    /// Names of failing checks. With `required` detection unavailable this
    /// lists ALL failing checks, not just protected ones.
    pub required_failing: Vec<String>,
    pub runs: Vec<CheckRun>,
}

/// One annotation a check left on the code (a compiler error, a failed
/// assertion with its file and line), as `GET /check-runs/{id}/annotations`
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CheckAnnotation {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// "failure" | "warning" | "notice"
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// What a failing check left behind, for an agent fixing it: the tail of the
/// job's log with GitHub's per-line timestamps stripped, and its annotations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckLog {
    pub id: u64,
    /// The last `lines` lines of the job log. `None` when the check has no
    /// job log to read (not a GitHub Actions job, or the log has expired);
    /// `note` then says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_tail: Option<String>,
    /// True when the log had more lines than were returned.
    pub truncated: bool,
    pub annotations: Vec<CheckAnnotation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One GitHub Actions workflow run, as the agent's `run_list` answers: what
/// agents read through `gh run list --branch … --workflow …` to see whether
/// a deploy ran after a merge, which `pr_checks` cannot say once there is no
/// pull request to ask about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkflowRun {
    pub id: u64,
    /// The workflow's name (`name:` in its file).
    pub name: String,
    /// The workflow file, as `run_list`'s `workflow` filter and
    /// `workflow_dispatch` take it (`ci.yml`), without the `.github/workflows/` prefix.
    pub workflow: String,
    /// The run's title: the commit subject or the PR title.
    pub title: String,
    pub branch: String,
    pub head_sha: String,
    /// What triggered it: `push`, `pull_request`, `workflow_dispatch`, `schedule`, …
    pub event: String,
    /// "queued" | "in_progress" | "completed" | "waiting" | …
    pub status: String,
    /// "success" | "failure" | "cancelled" | "skipped" | "timed_out" | … —
    /// `None` until completed.
    pub conclusion: Option<String>,
    pub url: String,
    pub run_number: u64,
    pub run_attempt: u64,
    /// ISO-8601 as GitHub gives them.
    pub created_at: String,
    pub updated_at: String,
}

/// What `run_list` asks for. Every field optional but the cap.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunListFilter {
    pub branch: Option<String>,
    /// A workflow file name (`ci.yml`) or numeric id; the runs of that
    /// workflow only.
    pub workflow: Option<String>,
    /// GitHub's `status` filter: a status (`queued`, `in_progress`,
    /// `completed`, …) or a conclusion (`success`, `failure`, …).
    pub status: Option<String>,
    pub event: Option<String>,
    pub limit: u32,
}

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    pub download_count: u64,
    /// The browser download URL.
    pub url: String,
}

/// A GitHub release, as the agent's `release_list` and `release_view`
/// answer: what agents read through `gh release list` / `view` to see what
/// shipped last and what a tag carries.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Release {
    pub id: u64,
    pub tag: String,
    /// The release title; empty when it was published without one.
    pub name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub url: String,
    pub author: String,
    /// The branch or commit the tag was cut from.
    pub target: String,
    /// ISO-8601 as GitHub gives them; `published_at` is absent for a draft.
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// The release notes. Left out of `release_list` rows; `release_view`
    /// carries them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

/// Where a review thread came from: GitHub (a human or a bot like Greptile on
/// the PR) or Fletch's own local reviewer, whose threads live in the host
/// database (`review::store`) and exist with or without a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSource {
    Github,
    Local,
}

/// How much a local reviewer's finding matters. GitHub threads carry none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    High,
    Medium,
    Low,
}

impl ReviewSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewSeverity::High => "high",
            ReviewSeverity::Medium => "medium",
            ReviewSeverity::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" => Some(ReviewSeverity::High),
            "medium" => Some(ReviewSeverity::Medium),
            "low" => Some(ReviewSeverity::Low),
            _ => None,
        }
    }
}

/// One unresolved PR review thread, flattened to its root comment. Surfaced
/// in the Git panel so review feedback (Greptile, other bots, humans, and the
/// local reviewer) is visible without leaving the app, with a quick action to
/// hand it to the coding agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrComment {
    /// The review thread's node id — what `resolveReviewThread` /
    /// `addPullRequestReviewThreadReply` address, so an agent can act on the
    /// thread and not merely read it. A local thread's is `local:<uuid>`.
    pub id: String,
    /// GitHub, or the local reviewer (`review::store`).
    pub source: ThreadSource,
    /// The local reviewer's severity; `None` for a GitHub thread.
    pub severity: Option<ReviewSeverity>,
    /// Which local review round raised it; `None` for a GitHub thread.
    pub round: Option<u32>,
    /// Comment author's login.
    pub author: String,
    /// True when the author is a GitHub App / bot (`__typename == "Bot"`).
    /// Bots like Greptile already phrase their comments for an AI, so the UI
    /// inserts them as-is; human comments get a file/line context wrapper.
    pub is_bot: bool,
    pub body: String,
    /// File the thread is anchored to. `None` for an unanchored thread (e.g.
    /// the line was deleted).
    pub path: Option<String>,
    pub line: Option<u32>,
    /// Permalink to the thread on GitHub; `None` for a local thread.
    pub url: Option<String>,
    /// Replies after the root comment (thread length − 1, clamped at 0).
    pub replies: u32,
    /// The connected account posted the last comment on this still-open thread —
    /// i.e. we've had our say and are waiting on the human. Distinguishes a
    /// deliberate push-back (leave open, don't re-argue) from a thread that
    /// genuinely needs attention, without keeping any local state.
    pub we_replied_last: bool,
}

/// Unresolved review threads for a PR. Heavier than `pr_view` — polled on the
/// same slow cadence as `pr_checks` while a PR is open.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrComments {
    pub unresolved: Vec<PrComment>,
}

/// One comment on a PR's conversation tab (an issue comment, not a review
/// thread). Read newest-last for the agent's `pr_comments` op.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PrConversationComment {
    /// Author's login; empty for a deleted account (GitHub's "ghost").
    pub author: String,
    /// True when the author is a GitHub App / bot (`__typename == "Bot"`).
    pub is_bot: bool,
    pub body: String,
    /// ISO-8601 as GitHub gives it.
    pub created_at: String,
    /// Permalink to the comment.
    pub url: String,
}

/// A PR's branch pair. Read for callers that hold a PR number but no checkout
/// of its branch — the roadmap board, whose items are built in disposable run
/// repos: `head` is the branch a fix agent must fork from to work *on* the PR,
/// and `base` is what the merge gate's "behind / conflicts with X" names.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrRefs {
    pub head: String,
    pub base: String,
}

/// The Git panel's fast-tick PR read: current state plus CI, both from
/// ETag-conditional REST in one pass (see `github::live`). `checks` is
/// `None` when the commit reads didn't resolve — distinct from a rollup of
/// zero checks, so a transient failure never blanks a passing tint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrLive {
    pub state: PrState,
    pub checks: Option<PrChecks>,
}

/// The repository behind a checkout's origin, as the agent's `repo_view`
/// answers: what agents asked `GET /repos/{o}/{r}` for before deciding how
/// to land work — visibility, the default branch, the merge methods allowed,
/// whether merged branches are deleted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RepoInfo {
    pub full_name: String,
    pub url: String,
    pub private: bool,
    pub archived: bool,
    pub default_branch: String,
    pub allow_squash_merge: bool,
    pub allow_merge_commit: bool,
    pub allow_rebase_merge: bool,
    pub delete_branch_on_merge: bool,
    pub has_issues: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// GitHub connection state. Drives the New Project UI and readiness rows.
/// `installed` is a legacy of the gh-CLI era kept for IPC compatibility —
/// there is no binary to install anymore, so it is always `true`; what
/// matters now is `authenticated` (a valid app token).
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub login: Option<String>,
}

/// One repo for the New Project clone picker.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GhRepoSummary {
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: String,
}
