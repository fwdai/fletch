//! Linear operations over the GraphQL API with the app-managed API key —
//! the second issue-tracker source behind `crate::issues` (GitHub was the
//! first). Same posture as `crate::github`: transport lives in [`client`],
//! endpoint knowledge and pure parsers live here, and read ops degrade to
//! `Ok(None)` when not connected so poll paths stay quiet.

pub mod client;

use serde_json::{json, Value};

use crate::error::Result;
use crate::issues::{IssueComment, IssueSource, TrackerIssue, TrackerLabel};

pub use client::{seed_token, set_token, TOKEN_SETTING};

/// One Linear team, for the Project Settings team picker. `id` is the UUID
/// the issues query filters by; `key` is the human prefix (`ENG`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinearTeam {
    pub id: String,
    pub key: String,
    pub name: String,
}

/// Validate an API key by asking who it belongs to. Used by connect (with the
/// pasted key, *before* persisting it). Returns the user's display name.
pub async fn viewer(api_key: String) -> Result<String> {
    let client = client::Client::with_key(api_key)?;
    let data = client
        .graphql("query { viewer { name email } }", json!({}))
        .await?;
    Ok(data["viewer"]["name"]
        .as_str()
        .or_else(|| data["viewer"]["email"].as_str())
        .unwrap_or_default()
        .to_string())
}

/// The workspace's teams, for the per-project team picker.
pub async fn teams() -> Result<Vec<LinearTeam>> {
    let client = client::Client::new()?;
    let data = client
        .graphql(
            "query { teams(first: 100) { nodes { id key name } } }",
            json!({}),
        )
        .await?;
    Ok(parse_teams(&data))
}

fn parse_teams(data: &Value) -> Vec<LinearTeam> {
    data["teams"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    Some(LinearTeam {
                        id: n["id"].as_str().filter(|s| !s.is_empty())?.to_string(),
                        key: n["key"].as_str().unwrap_or_default().to_string(),
                        name: n["name"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Open, relevant issues for a team, newest-updated first: not
/// completed/canceled, and unassigned or assigned to the key's user (`isMe`)
/// — never someone else's work. Both rules run server-side in the filter,
/// mirroring `github::issue_list`'s relevance contract. `Ok(None)` on any
/// degradation — no key, API/transport error — the same read-op contract as
/// `github::issue_list`, so the inbox and picker quietly show nothing
/// instead of erroring.
pub async fn issue_list(team_id: &str, limit: u32) -> Result<Option<Vec<TrackerIssue>>> {
    let client = match client::Client::new() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    const QUERY: &str = r#"
        query Issues($teamId: String!, $first: Int!) {
          team(id: $teamId) {
            issues(
              first: $first,
              orderBy: updatedAt,
              filter: {
                state: { type: { nin: ["completed", "canceled"] } },
                or: [
                  { assignee: { null: true } },
                  { assignee: { isMe: { eq: true } } }
                ]
              }
            ) {
              nodes {
                identifier
                title
                description
                url
                updatedAt
                assignee { displayName }
                labels { nodes { name color } }
              }
            }
          }
        }
    "#;
    let vars = json!({ "teamId": team_id, "first": limit.min(100) });
    match client.graphql(QUERY, vars).await {
        Ok(data) => Ok(Some(parse_issue_list(&data))),
        Err(_) => Ok(None),
    }
}

/// The comments on one issue, for the composed brief's discussion section.
/// `issue(id:)` accepts the human identifier ("ENG-123") in addition to the
/// UUID — exactly the `key` we store. Same degradation contract as
/// [`issue_list`]: `Ok(None)` on no key or API/transport error.
pub async fn issue_comments(key: &str, limit: u32) -> Result<Option<Vec<IssueComment>>> {
    let client = match client::Client::new() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    const QUERY: &str = r#"
        query IssueComments($id: String!) {
          issue(id: $id) {
            comments(first: 100) {
              nodes { body createdAt user { displayName } }
            }
          }
        }
    "#;
    match client.graphql(QUERY, json!({ "id": key })).await {
        Ok(data) => Ok(Some(parse_comment_list(&data, limit as usize))),
        Err(_) => Ok(None),
    }
}

/// Parse the comments payload into normalized comments ordered oldest-first
/// (the API's node order isn't contractual, so sort by `createdAt`), keeping
/// the trailing `limit`. Pure, so it's unit-tested without the network.
fn parse_comment_list(data: &Value, limit: usize) -> Vec<IssueComment> {
    let mut dated: Vec<(i64, IssueComment)> = data["issue"]["comments"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    let body = n["body"]
                        .as_str()
                        .map(str::trim)
                        .filter(|b| !b.is_empty())?
                        .to_string();
                    Some((
                        time_ms(n, "createdAt").unwrap_or(0),
                        IssueComment {
                            author: n["user"]["displayName"].as_str().map(str::to_string),
                            body,
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    dated.sort_by_key(|(t, _)| *t);
    crate::issues::keep_last(dated.into_iter().map(|(_, c)| c).collect(), limit)
}

/// ISO-8601 timestamp → ms-epoch (mirror of `github::query::gh_time_ms`).
fn time_ms(node: &Value, field: &str) -> Option<i64> {
    let iso = node[field].as_str()?;
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|t| t.timestamp_millis())
}

/// One label node → [`TrackerLabel`], dropping a nameless entry and
/// normalizing Linear's `#rrggbb` to the app-wide no-`#` 6-hex form.
fn parse_label(node: &Value) -> Option<TrackerLabel> {
    let name = node["name"].as_str().filter(|s| !s.is_empty())?.to_string();
    Some(TrackerLabel {
        name,
        color: node["color"]
            .as_str()
            .map(|c| c.trim_start_matches('#'))
            .filter(|c| !c.is_empty())
            .map(str::to_string),
    })
}

/// Parse the issues payload into normalized issues, dropping nodes with no
/// identifier. Pure, so it's unit-tested without the network.
fn parse_issue_list(data: &Value) -> Vec<TrackerIssue> {
    data["team"]["issues"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|n| {
                    let key = n["identifier"]
                        .as_str()
                        .filter(|s| !s.is_empty())?
                        .to_string();
                    Some(TrackerIssue {
                        source: IssueSource::Linear,
                        key,
                        title: n["title"].as_str().unwrap_or_default().to_string(),
                        url: n["url"].as_str().unwrap_or_default().to_string(),
                        labels: n["labels"]["nodes"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(parse_label).collect())
                            .unwrap_or_default(),
                        assignee: n["assignee"]["displayName"].as_str().map(str::to_string),
                        updated_at: time_ms(n, "updatedAt"),
                        body: n["description"]
                            .as_str()
                            .filter(|b| !b.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The issues payload parses into normalized issues: identifier becomes
    /// the key, `#`-prefixed label colors are normalized, empty descriptions
    /// drop to None, and an identifier-less node is skipped.
    #[test]
    fn issue_list_parses_and_normalizes() {
        let data = json!({
            "team": { "issues": { "nodes": [
                {
                    "identifier": "ENG-123",
                    "title": "Login crashes on empty password",
                    "description": "Steps to reproduce…",
                    "url": "https://linear.app/acme/issue/ENG-123",
                    "updatedAt": "2024-01-02T03:04:05.000Z",
                    "assignee": { "displayName": "Ada" },
                    "labels": { "nodes": [
                        { "name": "Bug", "color": "#d73a4a" },
                        { "name": "", "color": "#ccc" }
                    ] }
                },
                { "title": "no identifier — dropped" }
            ] } }
        });
        let issues = parse_issue_list(&data);
        assert_eq!(issues.len(), 1, "the identifier-less node must be dropped");
        let it = &issues[0];
        assert_eq!(it.source, IssueSource::Linear);
        assert_eq!(it.key, "ENG-123");
        assert_eq!(it.assignee.as_deref(), Some("Ada"));
        assert_eq!(it.body.as_deref(), Some("Steps to reproduce…"));
        assert_eq!(it.labels.len(), 1, "nameless labels are dropped");
        assert_eq!(
            it.labels[0].color.as_deref(),
            Some("d73a4a"),
            "Linear's #-prefixed color must be normalized to GitHub's bare form"
        );
        assert!(it.updated_at.is_some());
    }

    /// A non-object payload (an error shape, null data) parses to empty
    /// rather than panicking.
    #[test]
    fn issue_list_bad_payload_is_empty() {
        assert!(parse_issue_list(&json!({ "team": null })).is_empty());
        assert!(parse_issue_list(&Value::Null).is_empty());
    }

    #[test]
    fn teams_parse_drops_idless_nodes() {
        let data = json!({ "teams": { "nodes": [
            { "id": "uuid-1", "key": "ENG", "name": "Engineering" },
            { "key": "OPS", "name": "no id — dropped" }
        ] } });
        let teams = parse_teams(&data);
        assert_eq!(teams.len(), 1);
        assert_eq!(teams[0].key, "ENG");
    }

    /// Comments parse oldest-first regardless of node order (sorted by
    /// `createdAt`), blank bodies are dropped, and capping keeps the newest.
    #[test]
    fn comment_list_sorts_drops_blank_and_keeps_newest() {
        let data = json!({ "issue": { "comments": { "nodes": [
            {
                "body": "latest decision",
                "createdAt": "2024-01-03T00:00:00.000Z",
                "user": { "displayName": "Ada" }
            },
            { "body": "  ", "createdAt": "2024-01-02T00:00:00.000Z" },
            {
                "body": "first question",
                "createdAt": "2024-01-01T00:00:00.000Z",
                "user": { "displayName": "Grace" }
            }
        ] } } });
        let all = parse_comment_list(&data, 10);
        assert_eq!(all.len(), 2, "the blank body must be dropped");
        assert_eq!(all[0].author.as_deref(), Some("Grace"));
        assert_eq!(all[1].body, "latest decision");

        let capped = parse_comment_list(&data, 1);
        assert_eq!(capped.len(), 1);
        assert_eq!(
            capped[0].body, "latest decision",
            "capping must keep the newest comment"
        );
    }

    /// A non-object payload (an error shape) parses to empty, not a panic.
    #[test]
    fn comment_list_tolerates_error_payloads() {
        assert!(parse_comment_list(&json!(null), 5).is_empty());
        assert!(parse_comment_list(&json!({ "issue": null }), 5).is_empty());
    }
}
