use serde_json::Value;

use super::GitDispatcher;
use crate::rpc::Response;

impl GitDispatcher {
    pub(super) async fn pr_threads(&self, id: &str, args: &Value) -> Response {
        let t = match self.target(id, args) {
            Ok(t) => t,
            Err(resp) => return resp,
        };
        let number = match crate::github::pr_view(&t.cwd).await {
            Ok(Some(pr)) => pr.number,
            Ok(None) => return Response::err(id, "pr_threads: no pull request for this branch"),
            Err(e) => return Response::err(id, format!("pr_threads: {e}")),
        };
        match crate::github::pr_threads_number(&t.cwd, None, number).await {
            Ok(Some(threads)) => match serde_json::to_string_pretty(&threads.unresolved) {
                Ok(json) => Response::ok(id, 0, json, String::new()),
                Err(e) => Response::err(id, format!("pr_threads: {e}")),
            },
            Ok(None) => Response::ok(id, 0, "[]".to_string(), String::new()),
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
    use serde_json::json;

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
}
