//! Account/discovery for the New Project flow: auth probe, repo listing,
//! clone, and create-and-push.

use std::path::Path;

use serde_json::json;

use crate::error::{Error, Result};

use super::client;
use super::pr::detailed_rest_error;
use super::query::require_current_branch;
use super::types::*;

/// GitHub connection probe. Never errors — no token, an invalid token, and a
/// network failure all report as not-authenticated fields, not failures.
pub async fn auth_status() -> Result<GhStatus> {
    let Ok(client) = client::Client::new() else {
        return Ok(GhStatus {
            installed: true,
            authenticated: false,
            login: None,
        });
    };
    match client.graphql("query{viewer{login}}", json!({})).await {
        Ok(data) => {
            let login = data["viewer"]["login"].as_str().map(str::to_string);
            Ok(GhStatus {
                installed: true,
                authenticated: login.is_some(),
                login,
            })
        }
        Err(_) => Ok(GhStatus {
            installed: true,
            authenticated: false,
            login: None,
        }),
    }
}

/// List repos the user can clone (most-recently-updated first): owned,
/// collaborator, and org repos. The New Project picker filters client-side.
pub async fn repo_list(limit: u32) -> Result<Vec<GhRepoSummary>> {
    let client = client::Client::new()?;
    let query = r#"query($first:Int!,$after:String){
  viewer{
    repositories(first:$first, after:$after,
                 orderBy:{field:UPDATED_AT,direction:DESC},
                 affiliations:[OWNER,COLLABORATOR,ORGANIZATION_MEMBER]){
      nodes{ nameWithOwner description isPrivate updatedAt }
      pageInfo{ hasNextPage endCursor }
    }
  }
}"#;
    let mut repos = Vec::new();
    let mut cursor: Option<String> = None;
    while (repos.len() as u32) < limit {
        let first = (limit - repos.len() as u32).min(100);
        let data = client
            .graphql(query, json!({ "first": first, "after": cursor }))
            .await?;
        let page = &data["viewer"]["repositories"];
        for n in page["nodes"].as_array().cloned().unwrap_or_default() {
            repos.push(GhRepoSummary {
                name_with_owner: n["nameWithOwner"].as_str().unwrap_or_default().to_string(),
                description: n["description"]
                    .as_str()
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                is_private: n["isPrivate"].as_bool().unwrap_or(false),
                updated_at: n["updatedAt"].as_str().unwrap_or_default().to_string(),
            });
        }
        if !page["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false) {
            break;
        }
        cursor = page["pageInfo"]["endCursor"].as_str().map(str::to_string);
    }
    Ok(repos)
}

/// The URL `git clone` should use for a repo spec: `owner/repo` becomes an
/// https github.com URL (authenticated via `git_auth_env`); full https/ssh
/// URLs pass through untouched (ssh uses the user's own keys).
fn clone_url(spec: &str) -> String {
    if spec.contains("://") || spec.starts_with("git@") {
        spec.to_string()
    } else {
        format!("https://github.com/{}.git", spec.trim_end_matches(".git"))
    }
}

/// Clone `spec` (an `owner/repo`, an https URL, or an ssh URL) into `target`.
/// No timeout — a large repo can legitimately take minutes; the caller
/// (`new_project::clone`) self-heals a wedged partial clone by removing it.
pub async fn repo_clone(spec: &str, target: &Path) -> Result<()> {
    let target = target
        .to_str()
        .ok_or_else(|| Error::InvalidPath(target.display().to_string()))?;
    let mut cmd = crate::git_dist::bare_command();
    cmd.args(["clone", &clone_url(spec), target]);
    for (k, v) in client::git_auth_env() {
        cmd.env(k, v);
    }
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(Error::Gh(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Create a GitHub repo from the existing git repo at `target` and push its
/// current branch. `target` must already be a git repo with at least one
/// commit. Serves both the New Project create flow and the git panel's
/// "Publish to GitHub" for a local-only project. Returns the repo's web URL.
pub async fn repo_create_and_push(
    target: &Path,
    name: &str,
    private: bool,
    description: Option<&str>,
) -> Result<String> {
    let client = client::Client::new()?;
    let mut body = json!({ "name": name, "private": private });
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        body["description"] = json!(desc);
    }
    let (status, resp) = client
        .rest(reqwest::Method::POST, "/user/repos", Some(&body))
        .await?;
    if !status.is_success() {
        return Err(Error::Gh(format!(
            "repo create failed: {}",
            detailed_rest_error(&resp),
        )));
    }
    let full_name = resp["full_name"]
        .as_str()
        .ok_or_else(|| Error::Gh("repo created but response had no full_name".into()))?;

    crate::git::remote_add(
        target,
        "origin",
        &format!("https://github.com/{full_name}.git"),
    )
    .await?;
    let branch = require_current_branch(target, "publish").await?;
    crate::git::push(target, &branch, false).await?;
    Ok(format!("https://github.com/{full_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{pr_checks, pr_comments, pr_list, pr_view, pr_view_number};

    #[test]
    fn clone_url_forms() {
        assert_eq!(
            clone_url("fwdai/fletch"),
            "https://github.com/fwdai/fletch.git"
        );
        assert_eq!(
            clone_url("https://github.com/fwdai/fletch.git"),
            "https://github.com/fwdai/fletch.git",
        );
        assert_eq!(
            clone_url("git@github.com:fwdai/fletch.git"),
            "git@github.com:fwdai/fletch.git",
        );
    }

    /// Live read-path check against the real GitHub API, using this repo's
    /// own checkout as the fixture. Read-only — never creates or merges
    /// anything. Ignored by default (needs a token + network); run with:
    ///
    ///   FLETCH_GITHUB_TOKEN=$(gh auth token) cargo test github_live -- --ignored
    #[tokio::test]
    #[ignore]
    // The guard must span the awaits — that's its purpose (no other test may
    // touch the token registry while live calls run). Single test, no
    // re-entrancy, so the lint's deadlock scenario can't occur.
    #[allow(clippy::await_holding_lock)]
    async fn github_live_read_ops() {
        let _guard = client::test_token_lock();
        let token =
            std::env::var("FLETCH_GITHUB_TOKEN").expect("set FLETCH_GITHUB_TOKEN to a token");
        client::set_token(Some(token));
        // cargo test runs in src-tauri; the repo root is one up.
        let repo = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();

        let status = auth_status().await.unwrap();
        assert!(status.authenticated, "token must authenticate");
        assert!(status.login.is_some(), "viewer login must resolve");

        let repos = repo_list(3).await.unwrap();
        assert!(!repos.is_empty(), "repo list must return something");
        assert!(repos.iter().all(|r| r.name_with_owner.contains('/')));

        // Branch-PR resolution against whatever branch is checked out: a PR
        // (if one exists) must parse into a coherent state, and the heavier
        // checks/comments lookups for the same branch must not error.
        if let Some(pr) = pr_view(&repo).await.unwrap() {
            assert!(pr.number > 0);
            assert!(pr.url.starts_with("https://github.com/"));
            let by_number = pr_view_number(&repo, None, pr.number)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(by_number.number, pr.number);
            let checks = pr_checks(&repo).await.unwrap();
            assert!(checks.is_some(), "checks must resolve for an existing PR");
            let _ = pr_comments(&repo).await.unwrap();
        }

        // A PR number that can't exist maps to None, not an error.
        assert!(pr_view_number(&repo, None, 999_999_999)
            .await
            .unwrap()
            .is_none());

        let prs = pr_list(&repo, 5).await.unwrap();
        assert!(prs.len() <= 5);

        client::set_token(None);
    }
}
