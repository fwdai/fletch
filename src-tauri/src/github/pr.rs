//! PR read/create/merge over REST + GraphQL, plus the `PrState` parser and
//! REST error detail helper shared with the repo module.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::{Error, Result};

use super::client;
use super::query::{
    branch_pr_nodes, branch_prs_query, gh_time_ms, graphql_opt, pick_branch_pr, repo_ref,
    require_current_branch, require_repo_ref, resolve_slug,
};
use super::types::*;

pub(crate) const PR_STATE_FIELDS: &str = "number url title mergeable createdAt mergedAt";

pub(crate) fn parse_pr_state(node: &Value) -> PrState {
    PrState {
        number: node["number"].as_u64().unwrap_or_default() as u32,
        url: node["url"].as_str().unwrap_or_default().to_string(),
        state: match node["state"].as_str() {
            Some("MERGED") => PrStatus::Merged,
            Some("CLOSED") => PrStatus::Closed,
            _ => PrStatus::Open,
        },
        title: node["title"].as_str().unwrap_or_default().to_string(),
        mergeable: node["state"].as_str() == Some("OPEN")
            && node["mergeable"].as_str() == Some("MERGEABLE"),
        opened_at: gh_time_ms(node, "createdAt"),
        merged_at: gh_time_ms(node, "mergedAt"),
    }
}

/// Fetch the current PR state for the branch checked out in `checkout`.
/// `Ok(None)` when the branch has no PR (or no token / non-GitHub origin).
pub async fn pr_view(checkout: &Path) -> Result<Option<PrState>> {
    let Some(branch) = crate::git::current_branch(checkout).await? else {
        return Ok(None);
    };
    pr_view_branch(checkout, &branch).await
}

/// Fetch the current PR state for an explicit head branch, regardless of the
/// branch checked out in `checkout`. Backs the detached-HEAD callers (a
/// workflow run's step checkout is detached; its branch lives only on the
/// remote after a detached push), so they can't rely on `current_branch`.
pub async fn pr_view_branch(checkout: &Path, branch: &str) -> Result<Option<PrState>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    let query = branch_prs_query(PR_STATE_FIELDS);
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "branch": branch }),
    )
    .await?
    else {
        return Ok(None);
    };
    Ok(pick_branch_pr(&branch_pr_nodes(&data)).map(parse_pr_state))
}

/// Fetch PR state by explicit PR number, regardless of the branch currently
/// checked out in `checkout`. This is the lookup that doesn't rely on branch
/// identity: once we've recorded a PR number for an agent we fetch by it, so a
/// recycled workspace/branch name can't resolve to a different (e.g. a prior
/// agent's merged) PR. `Ok(None)` when the PR can't be found.
///
/// `owner/repo` resolves from the checkout's origin, falling back to
/// `source_repo` (the repo the agent was spawned against — it shares the same
/// origin) when the checkout is broken or gone. A checkout casualty — a moved
/// root, a pruned linked worktree — must not sever a by-number lookup that
/// never needed the checkout's git state in the first place.
pub async fn pr_view_number(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrState>> {
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Ok(None);
    };
    let query = format!(
        r#"query($owner:String!,$repo:String!,$number:Int!){{
  repository(owner:$owner,name:$repo){{ pullRequest(number:$number){{ state {PR_STATE_FIELDS} }} }}
}}"#
    );
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "number": number }),
    )
    .await?
    else {
        return Ok(None);
    };
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Ok(None);
    }
    Ok(Some(parse_pr_state(node)))
}

/// Fetch a PR's current body text by number. `Ok(None)` when the PR can't be
/// found (or no token / non-GitHub origin). Same slug resolution as
/// [`pr_view_number`] — the source repo backs up a broken checkout.
pub(crate) async fn pr_body(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<String>> {
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Ok(None);
    };
    let query = r#"query($owner:String!,$repo:String!,$number:Int!){
  repository(owner:$owner,name:$repo){ pullRequest(number:$number){ body } }
}"#;
    let Some(data) = graphql_opt(
        query,
        json!({ "owner": owner, "repo": repo, "number": number }),
    )
    .await?
    else {
        return Ok(None);
    };
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Ok(None);
    }
    Ok(Some(node["body"].as_str().unwrap_or_default().to_string()))
}

/// Overwrite a PR's body by number (REST PATCH). Used by the multi-repo
/// PR-set cross-linker; callers are expected to have fetched the current body
/// via [`pr_body`] and edited it (sentinel replacement, never blind append).
pub(crate) async fn pr_update_body(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
    body: &str,
) -> Result<()> {
    let Some((owner, repo)) = resolve_slug(checkout, source_repo).await else {
        return Err(Error::Gh(
            "this repository's `origin` remote is not a GitHub repository".into(),
        ));
    };
    let client = client::Client::new()?;
    let (status, resp) = client
        .rest(
            reqwest::Method::PATCH,
            &format!("/repos/{owner}/{repo}/pulls/{number}"),
            Some(&json!({ "body": body })),
        )
        .await?;
    if !status.is_success() {
        return Err(Error::Gh(format!(
            "pr body update failed: {}",
            detailed_rest_error(&resp)
        )));
    }
    Ok(())
}

/// List open PRs for the repo at `checkout` (newest first), for the
/// composer's "#" mention autocomplete. Empty when not connected.
pub async fn pr_list(checkout: &Path, limit: u32) -> Result<Vec<PrSummary>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(Vec::new());
    };
    let query = r#"query($owner:String!,$repo:String!,$limit:Int!){
  repository(owner:$owner,name:$repo){
    pullRequests(states:[OPEN], first:$limit, orderBy:{field:CREATED_AT,direction:DESC}){
      nodes{ number title state }
    }
  }
}"#;
    let Some(data) = graphql_opt(
        query,
        json!({ "owner": owner, "repo": repo, "limit": limit.min(100) }),
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    Ok(branch_pr_nodes(&data)
        .iter()
        .map(|n| PrSummary {
            number: n["number"].as_u64().unwrap_or_default() as u32,
            title: n["title"].as_str().unwrap_or_default().to_string(),
            state: match n["state"].as_str() {
                Some("MERGED") => PrStatus::Merged,
                Some("CLOSED") => PrStatus::Closed,
                _ => PrStatus::Open,
            },
        })
        .collect())
}

/// Whether a create failure means a PR for this branch already exists (the
/// REST 422 message is "A pull request already exists for owner:branch.").
/// Used to make `pr_create` idempotent across retries.
fn pr_already_exists(message: &str) -> bool {
    message.to_lowercase().contains("already exists")
}

/// Create a PR for the branch checked out in `checkout`. When `title` is
/// empty, the last commit's subject/body fill in (what gh's `--fill` did).
pub async fn pr_create(checkout: &Path, title: &str, body: &str, base: &str) -> Result<PrState> {
    let branch = require_current_branch(checkout, "pr create").await?;
    pr_create_head(checkout, &branch, title, body, base).await
}

/// Create a PR with an explicit head branch — for callers whose checkout is on
/// a detached HEAD (e.g. a workflow run's step checkout, whose `wf/…` branch
/// exists only on the remote after a detached push) and so can't derive the
/// head from `current_branch`. When `title` is empty, the last commit's
/// subject/body fill in.
pub async fn pr_create_head(
    checkout: &Path,
    head: &str,
    title: &str,
    body: &str,
    base: &str,
) -> Result<PrState> {
    let (owner, repo) = require_repo_ref(checkout).await?;

    let (title, body) = if title.is_empty() {
        crate::git::last_commit_message(checkout).await?
    } else {
        (title.to_string(), body.to_string())
    };

    let client = client::Client::new()?;
    let (status, resp) = client
        .rest(
            reqwest::Method::POST,
            &format!("/repos/{owner}/{repo}/pulls"),
            Some(&json!({ "title": title, "body": body, "head": head, "base": base })),
        )
        .await?;

    if !status.is_success() {
        let message = detailed_rest_error(&resp);
        // Idempotency: a prior attempt may have created the PR but failed
        // before we could fetch it. On retry GitHub reports the branch
        // already has a PR — treat that as success by returning the existing
        // one, so the caller isn't stuck erroring forever over a PR that's
        // actually there.
        if pr_already_exists(&message) {
            if let Some(pr) = pr_view_branch(checkout, head).await? {
                return Ok(pr);
            }
        }
        return Err(Error::Gh(format!("pr create failed: {message}")));
    }

    // The create response carries no `mergeable` verdict yet (GitHub computes
    // it async) — fetch the same shape every other path returns.
    let number = resp["number"].as_u64().unwrap_or_default() as u32;
    match pr_view_number(checkout, None, number).await? {
        Some(pr) => Ok(pr),
        None => Err(Error::Gh("PR was created but could not be fetched".into())),
    }
}

/// REST error bodies often carry the actionable detail in `errors[]`, not
/// `message` (e.g. create's "A pull request already exists…"). Join both.
pub(crate) fn detailed_rest_error(body: &Value) -> String {
    let mut parts = vec![client::rest_error_message(body)];
    if let Some(errors) = body.get("errors").and_then(Value::as_array) {
        parts.extend(
            errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str).map(str::to_string)),
        );
    }
    parts.retain(|p| !p.is_empty());
    parts.join("; ")
}

/// Merge the current branch's open PR: enable auto-merge (merge commit) so it
/// lands when checks pass — or merge immediately when GitHub refuses auto-merge
/// because there's nothing to wait for ("clean status") or because the repo has
/// auto-merge disabled ("auto merge is not allowed"), matching `gh pr merge --auto`.
///
/// The direct-merge fallback is only taken when the base branch has no merge
/// queue (`isMergeQueueEnabled`). On a merge-queue branch we must never fall
/// back to `mergePullRequest`: for a caller allowed to merge directly that would
/// land the PR outside the queue and skip its integration checks, so we surface
/// GitHub's refusal instead.
pub async fn pr_merge(checkout: &Path) -> Result<()> {
    let (owner, repo) = require_repo_ref(checkout).await?;
    let branch = require_current_branch(checkout, "pr merge").await?;

    let client = client::Client::new()?;
    let query = branch_prs_query("id isMergeQueueEnabled");
    let data = client
        .graphql(
            &query,
            json!({ "owner": owner, "repo": repo, "branch": branch }),
        )
        .await?;
    let nodes = branch_pr_nodes(&data);
    let pr = pick_branch_pr(&nodes)
        .filter(|n| n["state"].as_str() == Some("OPEN"))
        .ok_or_else(|| Error::Gh("no open PR for this branch".into()))?;
    let id = pr["id"]
        .as_str()
        .ok_or_else(|| Error::Gh("no open PR for this branch".into()))?
        .to_string();
    let merge_queue = pr["isMergeQueueEnabled"].as_bool().unwrap_or(false);

    let auto = client
        .graphql(
            r#"mutation($id:ID!){
  enablePullRequestAutoMerge(input:{pullRequestId:$id, mergeMethod:MERGE}){ clientMutationId }
}"#,
            json!({ "id": id }),
        )
        .await;
    match auto {
        Ok(_) => Ok(()),
        // GitHub refused to *queue* an auto-merge, but on a branch with no merge
        // queue the PR can still be merged directly. Two refusals mean "just
        // merge now":
        //  - "clean status": nothing to wait for, the PR is already mergeable.
        //  - "auto merge is not allowed": the repo has auto-merge disabled entirely.
        // On a merge-queue branch we skip this and let the refusal surface, so a
        // direct merge never bypasses the queue's required integration checks.
        Err(Error::Gh(msg))
            if !merge_queue && {
                let m = msg.to_lowercase();
                m.contains("clean status") || m.contains("auto merge is not allowed")
            } =>
        {
            client
                .graphql(
                    r#"mutation($id:ID!){
  mergePullRequest(input:{pullRequestId:$id, mergeMethod:MERGE}){ clientMutationId }
}"#,
                    json!({ "id": id }),
                )
                .await?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_node(state: &str, number: u32, mergeable: &str) -> Value {
        json!({
            "number": number,
            "url": format!("https://github.com/o/r/pull/{number}"),
            "state": state,
            "title": format!("PR {number}"),
            "mergeable": mergeable,
        })
    }

    #[test]
    fn pr_state_open_mergeable() {
        let pr = parse_pr_state(&pr_node("OPEN", 42, "MERGEABLE"));
        assert!(matches!(pr.state, PrStatus::Open));
        assert!(pr.mergeable);
        assert_eq!(pr.number, 42);
    }

    #[test]
    fn pr_state_merged_and_closed_are_never_mergeable() {
        let merged = parse_pr_state(&pr_node("MERGED", 1, "MERGEABLE"));
        assert!(matches!(merged.state, PrStatus::Merged));
        assert!(!merged.mergeable);
        let closed = parse_pr_state(&pr_node("CLOSED", 2, "UNKNOWN"));
        assert!(matches!(closed.state, PrStatus::Closed));
        assert!(!closed.mergeable);
    }

    #[test]
    fn pr_state_unknown_state_defaults_to_open() {
        let pr = parse_pr_state(&pr_node("SOMETHING_NEW", 3, "MERGEABLE"));
        assert!(matches!(pr.state, PrStatus::Open));
    }

    #[test]
    fn detects_already_exists_failure() {
        // GitHub's real 422 message for a duplicate PR.
        assert!(pr_already_exists(
            "A pull request already exists for fwdai:feat."
        ));
        // An unrelated failure must not be mistaken for it.
        assert!(!pr_already_exists("Validation Failed"));
    }
}
