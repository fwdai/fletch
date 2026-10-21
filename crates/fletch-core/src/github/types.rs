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

/// One unresolved PR review thread, flattened to its root comment. Surfaced
/// in the Git panel so review feedback (Greptile, other bots, humans) is
/// visible without leaving the app, with a quick action to hand it to the
/// coding agent.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PrComment {
    /// The review thread's node id — what `resolveReviewThread` /
    /// `addPullRequestReviewThreadReply` address, so an agent can act on the
    /// thread and not merely read it.
    pub id: String,
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
    /// Permalink to the thread on GitHub.
    pub url: String,
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
