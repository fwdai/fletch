//! Review threads: read the open ones, answer one, resolve one. Threads come
//! from two sources and the ops treat them alike: GitHub's review threads on
//! the branch's PR, and the local reviewer's findings (`review::store`, ids
//! `local:…`), which exist with or without a PR. The GitHub read is like the
//! other PR reads — the repository is the user's, the checkout supplies only
//! its branch, and the reasons a read can answer nothing are told apart up
//! front (`pr_read_target`); a local thread never needs GitHub at all.

use serde_json::Value;

use super::pr_reads::{branch_pr, json_ok, positive_u32_arg};
use super::GitDispatcher;
use crate::github::PrComment;
use crate::review::{self, ReplyAuthor};
use crate::rpc::Response;

/// Why a local thread op could not run without an engine behind the dispatcher.
const NO_LOCAL_THREADS: &str = "local review threads are not available in this session";

impl GitDispatcher {
    /// The unresolved review threads of the checkout: the local reviewer's,
    /// then the branch's PR's (or `args.number`'s), each with the `id` that
    /// `reply_thread` and `resolve_thread` take. With local threads and no
    /// readable PR, the local ones alone; with neither, the PR read's reason.
    pub(super) async fn pr_threads(&self, id: &str, args: &Value) -> Response {
        let number = match positive_u32_arg(args, "number") {
            Ok(n) => n,
            Err(e) => return Response::err(id, format!("pr_threads: {e}")),
        };
        let local = match self.local_threads(id, args) {
            Ok(local) => local,
            Err(resp) => return resp,
        };
        let github = self.github_threads(id, args, number).await;
        match (local.is_empty(), github) {
            (true, Err(resp)) => resp,
            (false, Err(_)) => json_ok(id, "pr_threads", &local),
            (_, Ok(github)) => {
                let mut all = local;
                all.extend(github);
                json_ok(id, "pr_threads", &all)
            }
        }
    }

    /// The branch's PR's unresolved GitHub threads, or why they can't be read.
    async fn github_threads(
        &self,
        id: &str,
        args: &Value,
        number: Option<u32>,
    ) -> Result<Vec<PrComment>, Response> {
        let t = self.pr_read_target(id, "pr_threads", args).await?;
        let number = match number {
            Some(n) => n,
            None => branch_pr(id, "pr_threads", &t).await?.number,
        };
        match crate::github::pr_threads_number(&t.source_repo, None, number).await {
            Ok(Some(threads)) => Ok(threads.unresolved),
            Ok(None) => Err(Response::err(
                id,
                format!(
                    "pr_threads: pull request #{number} was not found in this repository, or \
                     GitHub did not answer; try again shortly"
                ),
            )),
            Err(e) => Err(Response::err(id, format!("pr_threads: {e}"))),
        }
    }

    /// The local reviewer's open threads for the checkout `args.repo` names.
    /// Empty without an engine (tests) rather than an error: a GitHub-only
    /// read must still work there.
    fn local_threads(&self, id: &str, args: &Value) -> Result<Vec<PrComment>, Response> {
        let Some((ctx, _)) = &self.review else {
            return Ok(Vec::new());
        };
        let key = self.review_key(id, args)?;
        let conn = ctx.db.lock();
        review::store::unresolved(&conn, &key)
            .map_err(|e| Response::err(id, format!("pr_threads: {e}")))
    }

    /// The store key of the checkout a request names: this agent, and the
    /// subdir unless it is the primary's (`None`).
    fn review_key(&self, id: &str, args: &Value) -> Result<review::Key, Response> {
        let (_, agent_id) = self.review.as_ref().expect("checked by the caller");
        let t = self.target(id, args)?;
        let subdir = t.subdir.filter(|s| Some(s) != self.default_subdir.as_ref());
        Ok((agent_id.clone(), subdir))
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
        if review::store::is_local(thread) {
            return self
                .local_thread_op(id, "reply_thread", args, thread, |conn| {
                    review::store::reply(conn, thread, ReplyAuthor::Agent, body)
                })
                .await;
        }
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
        if review::store::is_local(thread) {
            return self
                .local_thread_op(id, "resolve_thread", args, thread, |conn| {
                    review::store::resolve(conn, thread, ReplyAuthor::Agent)
                })
                .await;
        }
        match crate::github::pr_resolve_thread(thread).await {
            Ok(()) => Response::ok(id, 0, "resolved".to_string(), String::new()),
            Err(e) => Response::err(id, format!("resolve_thread: {e}")),
        }
    }

    /// Run `mutate` on a local thread of this agent's checkout, then tell the
    /// clients the checkout's threads changed. The answer names the op as
    /// GitHub's would ("replied" / "resolved").
    async fn local_thread_op(
        &self,
        id: &str,
        op: &str,
        args: &Value,
        thread: &str,
        mutate: impl FnOnce(&rusqlite::Connection) -> crate::error::Result<bool>,
    ) -> Response {
        let Some((ctx, agent_id)) = &self.review else {
            return Response::err(id, format!("{op}: {NO_LOCAL_THREADS}"));
        };
        let key = match self.review_key(id, args) {
            Ok(key) => key,
            Err(resp) => return resp,
        };
        let found = {
            let conn = ctx.db.lock();
            // Only this checkout's threads are the agent's to answer.
            match review::store::get(&conn, thread) {
                Ok(Some(row)) if row.agent_id == key.0 && row.subdir == key.1 => mutate(&conn),
                Ok(_) => Ok(false),
                Err(e) => Err(e),
            }
        };
        match found {
            Ok(true) => {}
            Ok(false) => {
                return Response::err(
                    id,
                    format!("{op}: no local review thread {thread:?} on this checkout"),
                )
            }
            Err(e) => return Response::err(id, format!("{op}: {e}")),
        }
        review::emit_threads_changed(ctx, agent_id, key.1.as_deref(), Vec::new()).await;
        let said = if op == "reply_thread" {
            "replied"
        } else {
            "resolved"
        };
        Response::ok(id, 0, said.to_string(), String::new())
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
#[path = "tests/review_threads.rs"]
mod engine_tests;

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

    /// Without an engine behind the dispatcher a local id is an error that
    /// says so, never a GitHub call with a made-up node id.
    #[tokio::test]
    async fn local_thread_ops_need_an_engine() {
        let td = tempfile::tempdir().unwrap();
        let (resp, fx) = dispatcher(td.path())
            .dispatch_inner(
                "t",
                "reply_thread",
                &json!({ "thread": "local:abc", "body": "done" }),
            )
            .await;
        assert_eq!(
            resp.error.as_deref(),
            Some("reply_thread: local review threads are not available in this session")
        );
        assert!(fx.is_empty());
    }
}
