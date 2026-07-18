//! Open-issue listing for the Home inbox, with the pure REST parsers.

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

use super::client;
use super::query::{gh_time_ms, repo_ref};
use super::types::*;

/// One label node from the REST issues payload → [`IssueLabel`], dropping a
/// nameless entry.
fn parse_issue_label(node: &Value) -> Option<IssueLabel> {
    let name = node["name"].as_str().filter(|s| !s.is_empty())?.to_string();
    Some(IssueLabel {
        name,
        color: node["color"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(str::to_string),
    })
}

fn parse_issue(node: &Value) -> IssueSummary {
    IssueSummary {
        number: node["number"].as_u64().unwrap_or_default() as u32,
        title: node["title"].as_str().unwrap_or_default().to_string(),
        url: node["html_url"].as_str().unwrap_or_default().to_string(),
        labels: node["labels"]
            .as_array()
            .map(|arr| arr.iter().filter_map(parse_issue_label).collect())
            .unwrap_or_default(),
        assignee: node["assignee"]["login"].as_str().map(str::to_string),
        updated_at: gh_time_ms(node, "updated_at"),
        body: node["body"]
            .as_str()
            .filter(|b| !b.is_empty())
            .map(str::to_string),
    }
}

/// Every assignee login on a REST issue node — the `assignees` array (an
/// issue can have several; the single `assignee` field only carries the
/// first), falling back to `assignee` when the array is absent.
fn node_assignees(node: &Value) -> Vec<String> {
    let list: Vec<String> = node["assignees"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n["login"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        node["assignee"]["login"]
            .as_str()
            .map(str::to_string)
            .into_iter()
            .collect()
    } else {
        list
    }
}

/// The inbox/picker relevance rule: an issue is offered only when it's
/// unassigned or the signed-in user is among its assignees (co-assignment
/// counts) — never solely someone else's work. Logins compare
/// case-insensitively (GitHub's are case-preserving but case-insensitive).
/// With no resolvable login (a transient viewer failure) only unassigned
/// issues pass: the hard requirement is that other people's issues stay out,
/// and the next poll restores the assigned-to-me set.
fn relevant_assignees(viewer: Option<&str>, assignees: &[String]) -> bool {
    assignees.is_empty()
        || viewer.is_some_and(|v| assignees.iter().any(|a| a.eq_ignore_ascii_case(v)))
}

/// Parse the REST `GET /issues` array into relevant summaries: pull requests
/// dropped (that endpoint returns both, and only a PR node carries a
/// `pull_request` key), then the assignee relevance rule applied. Pure, so
/// the filter/parse is unit-tested without the network.
fn parse_issue_list(body: &Value, viewer: Option<&str>) -> Vec<IssueSummary> {
    body.as_array()
        .map(|arr| {
            arr.iter()
                .filter(|n| n.get("pull_request").is_none())
                .filter(|n| relevant_assignees(viewer, &node_assignees(n)))
                .map(parse_issue)
                .collect()
        })
        .unwrap_or_default()
}

/// The authenticated user's login, from the in-process cache or one `viewer`
/// query (cached until the token changes). `None` when the lookup fails.
async fn viewer_login(client: &client::Client) -> Option<String> {
    if let Some(login) = client::cached_login() {
        return Some(login);
    }
    let data = client
        .graphql("query{viewer{login}}", serde_json::json!({}))
        .await
        .ok()?;
    let login = data["viewer"]["login"].as_str()?.to_string();
    client::set_cached_login(Some(login.clone()));
    Some(login)
}

/// List open, relevant issues for the repo at `checkout` — unassigned or
/// assigned to the signed-in user (see [`relevant_assignee`]) — newest-updated
/// first, for the Home inbox and composer picker. `Ok(None)` on any
/// degradation (no token, non-GitHub origin, rate-limit pause, transport/HTTP
/// error) — the same read-op contract the PR lookups use, so the section
/// quietly disappears instead of erroring. A connected repo with no relevant
/// open issues returns `Ok(Some(vec![]))`.
pub async fn issue_list(checkout: &Path, limit: u32) -> Result<Option<Vec<IssueSummary>>> {
    // Honor the rate-limit pause like `graphql_opt` — serve nothing rather than
    // spend a REST call that would likely 403.
    if client::is_backing_off() {
        return Ok(None);
    }
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    // Background poll path: not being connected is a normal state, not an error.
    let client = match client::Client::new() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    // Fetch a full page (not just `limit`): the assignee filter below runs
    // client-side, so a window of someone-else's issues must not starve the
    // relevant ones out of the requested count.
    let path =
        format!("/repos/{owner}/{repo}/issues?state=open&sort=updated&direction=desc&per_page=100");
    let (status, body) = match client.rest_get_observed(&path).await {
        Ok(pair) => pair,
        Err(_) => return Ok(None),
    };
    if !status.is_success() {
        return Ok(None);
    }
    let viewer = viewer_login(&client).await;
    let mut issues = parse_issue_list(&body, viewer.as_deref());
    issues.truncate(limit as usize);
    Ok(Some(issues))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The REST `/issues` array mixes issues and pull requests; only PR nodes
    /// carry a `pull_request` key. Parsing must drop those and keep issue
    /// fields (labels, assignee, body).
    #[test]
    fn issue_list_drops_pull_requests_and_parses_fields() {
        let body = json!([
            {
                "number": 12,
                "title": "Login crashes on empty password",
                "html_url": "https://github.com/o/r/issues/12",
                "updated_at": "2024-01-02T03:04:05Z",
                "assignee": { "login": "octocat" },
                "labels": [
                    { "name": "bug", "color": "d73a4a" },
                    { "name": "", "color": "ccc" },
                    { "color": "no-name" }
                ],
                "body": "Steps to reproduce…"
            },
            {
                "number": 13,
                "title": "A PR masquerading as an issue",
                "html_url": "https://github.com/o/r/pull/13",
                "pull_request": { "url": "https://api.github.com/…/pulls/13" }
            }
        ]);
        let issues = parse_issue_list(&body, Some("octocat"));
        assert_eq!(
            issues.len(),
            1,
            "the pull_request node must be filtered out"
        );
        let it = &issues[0];
        assert_eq!(it.number, 12);
        assert_eq!(it.assignee.as_deref(), Some("octocat"));
        assert_eq!(it.body.as_deref(), Some("Steps to reproduce…"));
        assert_eq!(it.labels.len(), 1, "nameless labels are dropped");
        assert_eq!(it.labels[0].name, "bug");
        assert_eq!(it.labels[0].color.as_deref(), Some("d73a4a"));
        assert!(it.updated_at.is_some());
    }

    /// A non-array payload (an error object, an empty response) parses to an
    /// empty list rather than panicking.
    #[test]
    fn issue_list_non_array_is_empty() {
        assert!(parse_issue_list(&json!({ "message": "Not Found" }), None).is_empty());
        assert!(parse_issue_list(&Value::Null, None).is_empty());
    }

    /// Relevance: unassigned issues always pass; assigned issues pass only
    /// when the signed-in user is among the assignees (co-assignment counts,
    /// logins compare case-insensitively); with no resolvable login only
    /// unassigned issues pass, so someone else's work never appears.
    #[test]
    fn relevance_keeps_unassigned_and_mine_only() {
        let mine = vec!["octocat".to_string()];
        let mine_cased = vec!["OctoCat".to_string()];
        let theirs = vec!["alice".to_string()];
        let shared = vec!["alice".to_string(), "octocat".to_string()];
        assert!(relevant_assignees(Some("octocat"), &[]));
        assert!(relevant_assignees(Some("octocat"), &mine));
        assert!(relevant_assignees(Some("octocat"), &mine_cased));
        assert!(!relevant_assignees(Some("octocat"), &theirs));
        assert!(
            relevant_assignees(Some("octocat"), &shared),
            "co-assignment with a teammate still counts as mine"
        );
        assert!(relevant_assignees(None, &[]));
        assert!(!relevant_assignees(None, &mine));
    }

    /// The parse-level filter reads the full `assignees` array (the single
    /// `assignee` field only carries the first), so a co-assigned issue is
    /// kept and a solely-someone-else's issue is dropped.
    #[test]
    fn parse_filters_by_assignees_array() {
        let body = json!([
            {
                "number": 1,
                "title": "unassigned",
                "html_url": "https://github.com/o/r/issues/1",
                "assignees": []
            },
            {
                "number": 2,
                "title": "co-assigned to me (listed second)",
                "html_url": "https://github.com/o/r/issues/2",
                "assignee": { "login": "alice" },
                "assignees": [ { "login": "alice" }, { "login": "octocat" } ]
            },
            {
                "number": 3,
                "title": "someone else's",
                "html_url": "https://github.com/o/r/issues/3",
                "assignee": { "login": "alice" },
                "assignees": [ { "login": "alice" } ]
            }
        ]);
        let numbers: Vec<u32> = parse_issue_list(&body, Some("octocat"))
            .iter()
            .map(|i| i.number)
            .collect();
        assert_eq!(numbers, vec![1, 2]);
    }
}
