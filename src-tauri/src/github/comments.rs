//! Unresolved PR review threads: the branch lookup and the pure flattener.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::Result;

use super::query::{branch_pr_nodes, branch_prs_query, graphql_opt, pick_branch_pr, repo_ref};
use super::types::*;

/// Fetch the unresolved review threads for the current branch's PR — one
/// GraphQL call (threads inline with the branch-PR lookup; the gh version
/// needed two). `Ok(None)` when there is no PR; the command layer maps both
/// `None` and `Err` to "comments unavailable".
pub async fn pr_comments(checkout: &Path) -> Result<Option<PrComments>> {
    let Some((owner, repo)) = repo_ref(checkout).await else {
        return Ok(None);
    };
    let Some(branch) = crate::git::current_branch(checkout).await? else {
        return Ok(None);
    };
    let query = branch_prs_query(
        r#"reviewThreads(first:100){
             nodes{
               isResolved
               isOutdated
               comments(first:1){
                 totalCount
                 nodes{ author{ login __typename } body path line url }
               }
             }
           }"#,
    );
    let Some(data) = graphql_opt(
        &query,
        json!({ "owner": owner, "repo": repo, "branch": branch }),
    )
    .await?
    else {
        return Ok(None);
    };
    let nodes = branch_pr_nodes(&data);
    let Some(pr) = pick_branch_pr(&nodes) else {
        return Ok(None);
    };
    let threads = pr["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(Some(PrComments {
        unresolved: parse_review_threads(&threads),
    }))
}

/// Flatten review-thread nodes into the root comment of each *unresolved,
/// non-outdated* thread. Pure — unit tested against captured fixtures.
fn parse_review_threads(nodes: &[Value]) -> Vec<PrComment> {
    nodes
        .iter()
        .filter(|t| {
            !t["isResolved"].as_bool().unwrap_or(false)
                && !t["isOutdated"].as_bool().unwrap_or(false)
        })
        .filter_map(|t| {
            let comments = &t["comments"];
            let root = comments["nodes"].get(0)?;
            let total = comments["totalCount"].as_u64().unwrap_or(1);
            Some(PrComment {
                author: root["author"]["login"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string(),
                is_bot: root["author"]["__typename"].as_str() == Some("Bot"),
                body: root["body"].as_str().unwrap_or_default().to_string(),
                path: root["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                line: root["line"].as_u64().map(|n| n as u32),
                url: root["url"].as_str().unwrap_or_default().to_string(),
                replies: total.saturating_sub(1) as u32,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_threads_fixture() -> Vec<Value> {
        serde_json::from_str(
            r#"[
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"greptileai","__typename":"Bot"},
                 "body":"Consider handling the null case here.",
                 "path":"src/foo.rs","line":42,
                 "url":"https://github.com/o/r/pull/1#discussion_r1"}]}},
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":3,"nodes":[
                {"author":{"login":"alice","__typename":"User"},
                 "body":"Can we rename this?",
                 "path":"src/bar.rs","line":7,
                 "url":"https://github.com/o/r/pull/1#discussion_r2"}]}},
              {"isResolved":true,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"bob","__typename":"User"},"body":"resolved one",
                 "path":"src/baz.rs","line":1,"url":"u3"}]}},
              {"isResolved":false,"isOutdated":true,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"carol","__typename":"User"},"body":"stale one",
                 "path":"src/qux.rs","line":1,"url":"u4"}]}},
              {"isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"dave","__typename":"User"},"body":"unanchored",
                 "path":null,"line":null,"url":"u5"}]}}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn review_threads_keep_only_unresolved_active() {
        let comments = parse_review_threads(&review_threads_fixture());
        // Resolved + outdated dropped; 3 remain (greptile, alice, dave).
        assert_eq!(comments.len(), 3);
        assert!(comments
            .iter()
            .all(|c| c.author != "bob" && c.author != "carol"));
    }

    #[test]
    fn review_threads_flag_bots_and_count_replies() {
        let comments = parse_review_threads(&review_threads_fixture());
        let greptile = comments.iter().find(|c| c.author == "greptileai").unwrap();
        assert!(greptile.is_bot);
        assert_eq!(greptile.replies, 0);
        assert_eq!(greptile.path.as_deref(), Some("src/foo.rs"));
        assert_eq!(greptile.line, Some(42));

        let alice = comments.iter().find(|c| c.author == "alice").unwrap();
        assert!(!alice.is_bot);
        assert_eq!(alice.replies, 2); // totalCount 3 − root
    }

    #[test]
    fn review_threads_tolerate_null_anchor() {
        let comments = parse_review_threads(&review_threads_fixture());
        let dave = comments.iter().find(|c| c.author == "dave").unwrap();
        assert_eq!(dave.path, None);
        assert_eq!(dave.line, None);
    }
}
