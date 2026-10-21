//! Review threads: read the open ones, answer one, resolve one. The read is
//! like the other PR reads — the repository is the user's, the checkout
//! supplies only its branch, and the reasons a read can answer nothing are
//! told apart up front (`pr_read_target`).

use serde_json::Value;

use super::pr_reads::{branch_pr, json_ok, positive_u32_arg};
use super::GitDispatcher;
use crate::rpc::Response;

impl GitDispatcher {
    /// The unresolved review threads of the branch's PR (or `args.number`),
    /// each with the `id` that `reply_thread` and `resolve_thread` take.
    pub(super) async fn pr_threads(&self, id: &str, args: &Value) -> Response {
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("pr_threads: {e}")),
        };
        let t = match self.pr_read_target(id, "pr_threads", args).await {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match number {
            Some(n) => n,
            None => match branch_pr(id, "pr_threads", &t).await {
                Ok(pr) => pr.number,
                Err(resp) => return resp,
            },
        };
        match crate::github::pr_threads_number(&t.source_repo, None, number).await {
            Ok(Some(threads)) => json_ok(id, "pr_threads", &threads.unresolved),
            Ok(None) => Response::err(
                id,
                format!(
                    "pr_threads: pull request #{number} was not found in this repository, or \
                     GitHub did not answer; try again shortly"
                ),
            ),
            Err(e) => Response::err(id, format!("pr_threads: {e}")),
        }
    }

    pub(super) async fn reply_thread(&self, id: &str, args: &Value) -> Response {
        let (Some(thread), Some(body)) =
            (thread_arg(args), args.get("body").and_then(|v| v.as_str()))
        else {
            return Response::err(
                id,
                "reply_thread requires `thread` (the thread `id` from pr_threads) and `body`",
            );
        };
        match crate::github::pr_reply_thread(thread, body).await {
            Ok(()) => Response::ok(id, 0, "replied".to_string(), String::new()),
            Err(e) => Response::err(id, format!("reply_thread: {e}")),
        }
    }

    /// Separate from `reply_thread` so a disagreement can be left open.
    pub(super) async fn resolve_thread(&self, id: &str, args: &Value) -> Response {
        let Some(thread) = thread_arg(args) else {
            return Response::err(
                id,
                "resolve_thread requires `thread` (the thread `id` from pr_threads)",
            );
        };
        match crate::github::pr_resolve_thread(thread).await {
            Ok(()) => Response::ok(id, 0, "resolved".to_string(), String::new()),
            Err(e) => Response::err(id, format!("resolve_thread: {e}")),
        }
    }
}

/// The review thread a mutation addresses. `pr_threads` hands the agent the
/// node id under `id`, so that spelling and `thread_id` are read as well as
/// `thread`: the transcripts showed agents failing on the name alone, in more
/// than fifty sessions, and falling back to the GitHub CLI.
pub(super) fn thread_arg(args: &Value) -> Option<&str> {
    ["thread", "id", "thread_id"]
        .iter()
        .find_map(|key| args.get(key).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::thread_arg;
    use crate::rpc::git::test_support::*;
    use serde_json::{json, Value};

    #[test]
    fn thread_is_read_under_every_name_the_playbook_uses() {
        assert_eq!(thread_arg(&json!({ "thread": "PRRT_1" })), Some("PRRT_1"));
        assert_eq!(thread_arg(&json!({ "id": "PRRT_2" })), Some("PRRT_2"));
        assert_eq!(
            thread_arg(&json!({ "thread_id": " PRRT_3 " })),
            Some("PRRT_3")
        );
        assert_eq!(
            thread_arg(&json!({ "thread": "PRRT_1", "id": "PRRT_2" })),
            Some("PRRT_1"),
            "`thread` wins when several are given"
        );
    }

    #[test]
    fn a_missing_or_blank_thread_is_none() {
        assert_eq!(thread_arg(&json!({})), None);
        assert_eq!(thread_arg(&json!({ "thread": "" })), None);
        assert_eq!(thread_arg(&json!({ "id": 42 })), None);
        assert_eq!(thread_arg(&json!({ "body": "x" })), None);
    }

    /// The read goes through the PR reads' preflight now: a bad number is
    /// refused first, and a checkout with no GitHub origin is told so instead
    /// of "no pull request for this branch".
    #[tokio::test]
    async fn pr_threads_validates_and_names_a_non_github_origin() {
        let td = tempfile::tempdir().unwrap();
        let (resp, fx) = dispatcher(td.path())
            .dispatch_inner("t", "pr_threads", &json!({ "number": 0 }))
            .await;
        assert!(resp
            .error
            .unwrap()
            .starts_with("pr_threads: `number` must be a positive integer"));
        assert!(fx.is_empty());

        let repo = td.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q", "-b", "feat/x"]);
        for args in [Value::Null, json!({ "number": 7 })] {
            let (resp, fx) = dispatcher(&repo)
                .dispatch_inner("t", "pr_threads", &args)
                .await;
            assert_eq!(
                resp.error.as_deref(),
                Some("pr_threads: origin is not a GitHub repository (or there is no origin)"),
                "{args}"
            );
            assert!(fx.is_empty());
        }
    }
}
