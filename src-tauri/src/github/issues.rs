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

/// Parse the REST `GET /issues` array into summaries, dropping pull requests —
/// that endpoint returns both, and only a PR node carries a `pull_request` key.
/// Pure, so the filter/parse is unit-tested without the network.
fn parse_issue_list(body: &Value) -> Vec<IssueSummary> {
    body.as_array()
        .map(|arr| {
            arr.iter()
                .filter(|n| n.get("pull_request").is_none())
                .map(parse_issue)
                .collect()
        })
        .unwrap_or_default()
}

/// List open issues for the repo at `checkout`, newest-updated first, for the
/// Home inbox. `Ok(None)` on any degradation (no token, non-GitHub origin,
/// rate-limit pause, transport/HTTP error) — the same read-op contract the PR
/// lookups use, so the section quietly disappears instead of erroring. A
/// connected repo with no open issues returns `Ok(Some(vec![]))`.
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
    let path = format!(
        "/repos/{owner}/{repo}/issues?state=open&sort=updated&direction=desc&per_page={}",
        limit.min(100)
    );
    let (status, body) = match client.rest_get_observed(&path).await {
        Ok(pair) => pair,
        Err(_) => return Ok(None),
    };
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(parse_issue_list(&body)))
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
        let issues = parse_issue_list(&body);
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
        assert!(parse_issue_list(&json!({ "message": "Not Found" })).is_empty());
        assert!(parse_issue_list(&Value::Null).is_empty());
    }
}
