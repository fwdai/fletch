//! Releases, read for the agent's `release_list` and `release_view` ops:
//! what shipped last, and what a tag carries. Reads only; publishing a
//! release stays with CI and the user.

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

use super::client;
use super::query::require_repo_ref;
use super::types::*;

/// GitHub's page cap for the releases list.
pub const RELEASE_LIST_MAX: u32 = 100;

/// The repository's releases, newest first, without their notes. `Ok(None)`
/// when GitHub answers anything but success (a repository the token cannot
/// see).
pub async fn release_list(checkout: &Path, limit: u32) -> Result<Option<Vec<Release>>> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;
    let per_page = limit.clamp(1, RELEASE_LIST_MAX).to_string();
    let path = client::with_query(
        &format!("/repos/{owner}/{repo}/releases"),
        &[("per_page", &per_page)],
    );
    let (status, body) = client.rest_get_observed(&path).await?;
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(parse_release_list(&body)))
}

/// One release by `tag`, or the latest published one (no draft, no
/// prerelease) when `tag` is `None`. `Ok(None)` when GitHub answers anything
/// but success: no such tag, no release yet, or a repository the token
/// cannot see.
pub async fn release_view(checkout: &Path, tag: Option<&str>) -> Result<Option<Release>> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let client = client::Client::new()?;
    // The tag is one encoded segment: `/` and `@` are legal in a tag name.
    let path = match tag {
        Some(tag) => client::with_segment(&format!("/repos/{owner}/{repo}/releases/tags"), tag),
        None => format!("/repos/{owner}/{repo}/releases/latest"),
    };
    let (status, body) = client.rest_get_observed(&path).await?;
    if !status.is_success() {
        return Ok(None);
    }
    Ok(Some(parse_release(&body, true)))
}

/// The REST releases array as rows, notes left out. Pure.
pub(crate) fn parse_release_list(body: &Value) -> Vec<Release> {
    body.as_array()
        .map(|arr| arr.iter().map(|r| parse_release(r, false)).collect())
        .unwrap_or_default()
}

/// One REST release object → [`Release`]; `with_body` keeps the notes.
pub(crate) fn parse_release(node: &Value, with_body: bool) -> Release {
    let text = |key: &str| node[key].as_str().unwrap_or_default().to_string();
    Release {
        id: node["id"].as_u64().unwrap_or_default(),
        tag: text("tag_name"),
        name: text("name"),
        draft: node["draft"].as_bool().unwrap_or(false),
        prerelease: node["prerelease"].as_bool().unwrap_or(false),
        url: text("html_url"),
        author: node["author"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        target: text("target_commitish"),
        created_at: text("created_at"),
        published_at: node["published_at"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        body: with_body.then(|| text("body")),
        assets: node["assets"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|a| ReleaseAsset {
                        name: a["name"].as_str().unwrap_or_default().to_string(),
                        size: a["size"].as_u64().unwrap_or_default(),
                        download_count: a["download_count"].as_u64().unwrap_or_default(),
                        url: a["browser_download_url"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node() -> Value {
        json!({
            "id": 31, "tag_name": "v1.2.0", "name": "Fletch 1.2", "draft": false,
            "prerelease": true, "html_url": "https://github.com/o/r/releases/tag/v1.2.0",
            "author": { "login": "octocat" }, "target_commitish": "main",
            "created_at": "2026-10-20T00:00:00Z", "published_at": "2026-10-21T00:00:00Z",
            "body": "## What's new\n- things",
            "assets": [
                { "name": "Fletch.dmg", "size": 1024, "download_count": 7,
                  "browser_download_url": "https://github.com/o/r/releases/download/v1.2.0/Fletch.dmg" }
            ]
        })
    }

    #[test]
    fn a_release_parses_whole_and_a_list_row_drops_the_notes() {
        let whole = parse_release(&node(), true);
        assert_eq!(whole.tag, "v1.2.0");
        assert_eq!(whole.name, "Fletch 1.2");
        assert!(whole.prerelease && !whole.draft);
        assert_eq!(whole.author, "octocat");
        assert_eq!(whole.target, "main");
        assert_eq!(whole.published_at.as_deref(), Some("2026-10-21T00:00:00Z"));
        assert_eq!(whole.body.as_deref(), Some("## What's new\n- things"));
        assert_eq!(whole.assets.len(), 1);
        assert_eq!(whole.assets[0].name, "Fletch.dmg");
        assert_eq!(whole.assets[0].download_count, 7);

        let rows = parse_release_list(&json!([node()]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body, None, "list rows carry no notes");
        assert!(parse_release_list(&json!({ "message": "Not Found" })).is_empty());
    }

    #[test]
    fn a_draft_has_no_published_time_and_a_sparse_node_still_parses() {
        let draft = parse_release(
            &json!({ "id": 1, "tag_name": "v2", "draft": true, "published_at": null }),
            true,
        );
        assert!(draft.draft);
        assert_eq!(draft.published_at, None);
        assert_eq!(draft.name, "");
        assert_eq!(draft.body.as_deref(), Some(""));
        assert!(draft.assets.is_empty());
    }
}
