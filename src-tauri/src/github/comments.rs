//! Unresolved PR review threads: the GraphQL selection, the by-number read,
//! the node extractor, and the pure flattener.

use std::path::Path;

use serde_json::Value;

use crate::error::Result;

use super::query::pr_node_by_number;
use super::types::*;

/// GraphQL selection for a PR's review threads. Reused by the branch lookup
/// (`pr_comments`) and the by-number detail read (`pr_detail_number`).
///
/// This is the app's most expensive selection by a wide margin: `comments` is
/// nested two connections deep, so under a `first:30` branch scan it bills
/// 30×100 = 3000 requests (~30 points), *regardless of how many threads the PR
/// actually has* — GraphQL charges the declared shape, not the result. Read it
/// by PR number wherever a number is known.
pub(crate) const PR_COMMENTS_FIELDS: &str = r#"reviewThreads(first:100){
     nodes{
       isResolved
       isOutdated
       comments(first:1){
         totalCount
         nodes{ author{ login __typename } body path line url }
       }
     }
   }"#;

/// Extract [`PrComments`] from a PR node carrying [`PR_COMMENTS_FIELDS`].
pub(crate) fn pr_comments_from_node(pr: &Value) -> PrComments {
    let threads = pr["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    PrComments {
        unresolved: parse_review_threads(&threads),
    }
}

/// Fetch a PR's unresolved review threads by number — the panel's slow tick.
///
/// This is the one panel read that must stay on GraphQL: thread *resolution*
/// (`isResolved`/`isOutdated`) has no REST equivalent, so the ETag-conditional
/// REST path in `live.rs` cannot serve it. By number it costs 1 point; there is
/// deliberately no branch-scan variant, which under `branch_prs_query`'s
/// `first:30` billed ~30 points a call and drained the hourly budget on its own.
///
/// `Ok(None)` when the PR or slug can't be resolved, or while a rate-limit
/// backoff is active.
pub async fn pr_threads_number(
    checkout: &Path,
    source_repo: Option<&Path>,
    number: u32,
) -> Result<Option<PrComments>> {
    let Some(node) = pr_node_by_number(checkout, source_repo, number, PR_COMMENTS_FIELDS).await?
    else {
        return Ok(None);
    };
    Ok(Some(pr_comments_from_node(&node)))
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
