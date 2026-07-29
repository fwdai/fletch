//! Unresolved PR review threads: the GraphQL selection, the by-number read,
//! the node extractor, and the pure flattener.

use std::path::Path;

use serde_json::{json, Value};

use crate::error::Result;

use super::client;
use super::query::pr_node_and_viewer;
use super::types::*;

/// GraphQL selection for a PR's review threads. Reused by the branch lookup
/// (`pr_comments`) and the by-number detail read (`pr_detail_number`).
///
/// This is the app's most expensive selection by a wide margin: `comments` is
/// nested two connections deep, so under a `first:30` branch scan it bills
/// 30×100 = 3000 requests (~30 points), *regardless of how many threads the PR
/// actually has* — GraphQL charges the declared shape, not the result. Read it
/// by PR number wherever a number is known.
///
/// `id` is the thread's node id — what `resolveReviewThread` /
/// `addPullRequestReviewThreadReply` address, so an agent can act on a thread
/// rather than just read it.
///
/// `comments(last:1)` is here to answer one question: did WE have the last word?
/// An agent that pushes back on a comment leaves the thread open deliberately,
/// and without this it would re-argue the same point on every poll, posting a
/// duplicate reply each time. Reading the last author off GitHub keeps that
/// stateless — it survives a restart, where a local "already disputed" set
/// wouldn't — and it distinguishes "we're waiting on the human" from "the human
/// answered, engage again".
pub(crate) const PR_COMMENTS_FIELDS: &str = r#"reviewThreads(first:100){
     nodes{
       id
       isResolved
       isOutdated
       comments(first:1){
         totalCount
         nodes{ author{ login __typename } body path line url }
       }
       lastComment: comments(last:1){
         nodes{ author{ login } }
       }
     }
   }"#;

/// Extract [`PrComments`] from a PR node carrying [`PR_COMMENTS_FIELDS`].
///
/// `viewer_login` is the connected account, used to decide `we_replied_last`.
/// `None` (login unknown) leaves the flag false everywhere, which is the safe
/// default: threads read as needing attention rather than as awaiting a human,
/// so nothing is silently parked.
pub(crate) fn pr_comments_from_node(pr: &Value, viewer_login: Option<&str>) -> PrComments {
    let threads = pr["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    PrComments {
        unresolved: parse_review_threads(&threads, viewer_login),
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
    let Some((node, viewer)) =
        pr_node_and_viewer(checkout, source_repo, number, PR_COMMENTS_FIELDS).await?
    else {
        return Ok(None);
    };
    Ok(Some(pr_comments_from_node(&node, viewer.as_deref())))
}

/// Flatten review-thread nodes into the root comment of each *unresolved,
/// non-outdated* thread. Pure — unit tested against captured fixtures.
fn parse_review_threads(nodes: &[Value], viewer_login: Option<&str>) -> Vec<PrComment> {
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
            // Only a *reply* can be ours in the sense that matters. A thread we
            // opened ourselves and nobody answered has us as the last author too,
            // but there is nothing to wait for there — so require more than one
            // comment before reading the last author as "we had the last word".
            let we_replied_last = total > 1
                && viewer_login.is_some_and(|me| {
                    t["lastComment"]["nodes"]
                        .get(0)
                        .and_then(|c| c["author"]["login"].as_str())
                        == Some(me)
                });
            Some(PrComment {
                id: t["id"].as_str().unwrap_or_default().to_string(),
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
                we_replied_last,
            })
        })
        .collect()
}

/// Mark a review thread resolved. Only ever a claim that the thread is
/// discharged — the agent fixed what it asked for, or answered it. A
/// disagreement is deliberately NOT resolved this way: pushing back leaves the
/// thread open for the human to settle.
pub async fn pr_resolve_thread(thread_id: &str) -> Result<()> {
    client::Client::new()?
        .graphql(
            r#"mutation($id:ID!){
  resolveReviewThread(input:{threadId:$id}){ thread{ id isResolved } }
}"#,
            json!({ "id": thread_id }),
        )
        .await?;
    Ok(())
}

/// Reply on a review thread. Every outcome carries one: a fix says what changed,
/// an answer answers, a push-back gives its reasoning. That reply is the whole
/// audit trail for an action nobody watched happen.
pub async fn pr_reply_thread(thread_id: &str, body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(crate::error::Error::Gh("reply body is empty".into()));
    }
    client::Client::new()?
        .graphql(
            r#"mutation($id:ID!,$body:String!){
  addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id, body:$body}){
    comment{ id }
  }
}"#,
            json!({ "id": thread_id, "body": body }),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_threads_fixture() -> Vec<Value> {
        serde_json::from_str(
            r#"[
              {"id":"T1","isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"greptileai","__typename":"Bot"},
                 "body":"Consider handling the null case here.",
                 "path":"src/foo.rs","line":42,
                 "url":"https://github.com/o/r/pull/1#discussion_r1"}]},
               "lastComment":{"nodes":[{"author":{"login":"greptileai"}}]}},
              {"id":"T2","isResolved":false,"isOutdated":false,"comments":{"totalCount":3,"nodes":[
                {"author":{"login":"alice","__typename":"User"},
                 "body":"Can we rename this?",
                 "path":"src/bar.rs","line":7,
                 "url":"https://github.com/o/r/pull/1#discussion_r2"}]},
               "lastComment":{"nodes":[{"author":{"login":"me"}}]}},
              {"isResolved":true,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"bob","__typename":"User"},"body":"resolved one",
                 "path":"src/baz.rs","line":1,"url":"u3"}]}},
              {"isResolved":false,"isOutdated":true,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"carol","__typename":"User"},"body":"stale one",
                 "path":"src/qux.rs","line":1,"url":"u4"}]}},
              {"id":"T5","isResolved":false,"isOutdated":false,"comments":{"totalCount":1,"nodes":[
                {"author":{"login":"me","__typename":"User"},"body":"unanchored",
                 "path":null,"line":null,"url":"u5"}]},
               "lastComment":{"nodes":[{"author":{"login":"me"}}]}}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn review_threads_keep_only_unresolved_active() {
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
        // Resolved + outdated dropped; 3 remain (greptile, alice, dave).
        assert_eq!(comments.len(), 3);
        assert!(comments
            .iter()
            .all(|c| c.author != "bob" && c.author != "carol"));
    }

    #[test]
    fn review_threads_flag_bots_and_count_replies() {
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
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
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
        let unanchored = comments.iter().find(|c| c.url == "u5").unwrap();
        assert_eq!(unanchored.path, None);
        assert_eq!(unanchored.line, None);
    }

    /// The stateless "don't re-argue" signal: a thread whose last comment is ours
    /// is a push-back awaiting a person, not work waiting to be done.
    #[test]
    fn review_threads_flag_the_thread_we_answered_last() {
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
        let alice = comments.iter().find(|c| c.author == "alice").unwrap();
        assert!(alice.we_replied_last, "we posted the last reply on T2");

        let greptile = comments.iter().find(|c| c.author == "greptileai").unwrap();
        assert!(!greptile.we_replied_last, "nobody has replied to T1 yet");
    }

    /// A single-comment thread we opened ourselves has us as last author too, but
    /// there is nothing to wait for — only a genuine *reply* parks a thread.
    #[test]
    fn a_thread_we_opened_and_nobody_answered_is_not_awaiting_us() {
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
        let ours = comments.iter().find(|c| c.url == "u5").unwrap();
        assert_eq!(ours.author, "me");
        assert!(!ours.we_replied_last, "totalCount 1 means no reply exists");
    }

    /// Unknown login must not park anything: false everywhere is the safe default.
    #[test]
    fn no_viewer_login_never_parks_a_thread() {
        let comments = parse_review_threads(&review_threads_fixture(), None);
        assert!(comments.iter().all(|c| !c.we_replied_last));
    }

    #[test]
    fn review_threads_carry_the_thread_id_for_mutations() {
        let comments = parse_review_threads(&review_threads_fixture(), Some("me"));
        assert_eq!(comments.iter().filter(|c| c.id.is_empty()).count(), 0);
        assert!(comments.iter().any(|c| c.id == "T1"));
    }
}
