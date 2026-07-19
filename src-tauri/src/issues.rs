//! Generalized issue-tracker surface: one normalized issue shape served by
//! per-source adapters — GitHub issues (over `crate::github`) and Linear
//! tickets (over `crate::linear`) today, more sources tomorrow. The Home
//! inbox and the composer's issue picker consume [`TrackerIssue`] only, so
//! adding a source never touches the UI plumbing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::github;

/// Live issue ref per agent ("123" / "ENG-123"), read by the git dispatcher
/// at `open_pr` time so a mid-session pick (the composer's issue picker in an
/// existing chat) reaches the PR trailer without rebuilding the dispatcher —
/// which is constructed once per session and would otherwise hold a stale
/// copy. Seeded from the workspace row at spawn/resume and updated by the
/// `set_agent_issue_ref` command alongside that row, which stays the durable
/// source of truth across restarts.
fn issue_ref_registry() -> &'static Mutex<HashMap<String, String>> {
    static REG: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Set (or, with `None`/blank, clear) an agent's live issue ref.
pub fn set_live_issue_ref(agent_id: &str, issue_ref: Option<String>) {
    let mut reg = issue_ref_registry().lock().unwrap();
    match issue_ref.filter(|s| !s.trim().is_empty()) {
        Some(r) => {
            reg.insert(agent_id.to_string(), r);
        }
        None => {
            reg.remove(agent_id);
        }
    }
}

pub fn live_issue_ref(agent_id: &str) -> Option<String> {
    issue_ref_registry().lock().unwrap().get(agent_id).cloned()
}

/// Which tracker an issue came from. Serialized lowercase — the frontend's
/// `IssueSource` union mirrors it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSource {
    Github,
    Linear,
}

/// One label on an issue. `color` is a 6-hex assignment with no leading `#`
/// (GitHub's native form; Linear's `#rrggbb` is normalized to it).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackerLabel {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// An open issue from any connected tracker — enough to list it, show its
/// labels/assignee, and seed a "Start work" brief (title + body + url).
/// `key` is the canonical reference persisted as a workspace's `issue_ref`
/// and consumed by the PR trailer: the bare number for GitHub (`"123"` →
/// `Closes #123`), the identifier for Linear (`"ENG-123"` → `Fixes ENG-123`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrackerIssue {
    pub source: IssueSource,
    pub key: String,
    pub title: String,
    pub url: String,
    pub labels: Vec<TrackerLabel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// `updatedAt` as ms-epoch, for ordering and the "updated N ago" hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// Issue body/description, carried so the brief composes without a
    /// second round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// GitHub adapter: [`github::IssueSummary`] → the normalized shape.
fn from_github(issue: github::IssueSummary) -> TrackerIssue {
    TrackerIssue {
        source: IssueSource::Github,
        key: issue.number.to_string(),
        title: issue.title,
        url: issue.url,
        labels: issue
            .labels
            .into_iter()
            .map(|l| TrackerLabel {
                name: l.name,
                color: l.color,
            })
            .collect(),
        assignee: issue.assignee,
        updated_at: issue.updated_at,
        body: issue.body,
    }
}

/// List open, relevant issues for a repo across every configured source,
/// newest-updated first, capped at `limit`. Every adapter enforces the same
/// relevance rule: not closed/completed, and unassigned or assigned to the
/// signed-in user — someone else's work never enters the inbox or picker.
/// Each adapter degrades to nothing on its own
/// failures (no token, non-GitHub origin, no Linear team configured, API
/// error) — the same quiet contract `github::issue_list` set — so one broken
/// source never blanks the others.
pub async fn issue_list(
    checkout: &Path,
    linear_team_id: Option<&str>,
    limit: u32,
) -> Vec<TrackerIssue> {
    let github_issues = github::issue_list(checkout, limit);
    let linear_issues = async {
        match linear_team_id.filter(|t| !t.trim().is_empty()) {
            Some(team) => crate::linear::issue_list(team, limit).await.ok().flatten(),
            None => None,
        }
    };
    let (gh, linear) = tokio::join!(github_issues, linear_issues);

    let mut issues: Vec<TrackerIssue> = gh
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(from_github)
        .chain(linear.unwrap_or_default())
        .collect();
    issues.sort_by_key(|i| std::cmp::Reverse(i.updated_at.unwrap_or(0)));
    issues.truncate(limit as usize);
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A later pick replaces the spawn seed, and a blank/None clears the tag
    /// (open_pr then appends no trailer).
    #[test]
    fn live_issue_ref_set_replace_clear() {
        set_live_issue_ref("issues-test-agent", Some("123".into()));
        assert_eq!(live_issue_ref("issues-test-agent").as_deref(), Some("123"));
        set_live_issue_ref("issues-test-agent", Some("ENG-9".into()));
        assert_eq!(
            live_issue_ref("issues-test-agent").as_deref(),
            Some("ENG-9")
        );
        set_live_issue_ref("issues-test-agent", Some("  ".into()));
        assert_eq!(live_issue_ref("issues-test-agent"), None);
    }
}
