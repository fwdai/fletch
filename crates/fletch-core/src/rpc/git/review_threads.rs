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
        let (Some(thread), Some(body)) = (
            args.get("thread").and_then(|v| v.as_str()),
            args.get("body").and_then(|v| v.as_str()),
        ) else {
            return Response::err(id, "reply_thread requires `thread` and `body`");
        };
        match crate::github::pr_reply_thread(thread, body).await {
            Ok(()) => Response::ok(id, 0, "replied".to_string(), String::new()),
            Err(e) => Response::err(id, format!("reply_thread: {e}")),
        }
    }

    /// Separate from `reply_thread` so a disagreement can be left open.
    pub(super) async fn resolve_thread(&self, id: &str, args: &Value) -> Response {
        let Some(thread) = args.get("thread").and_then(|v| v.as_str()) else {
            return Response::err(id, "resolve_thread requires `thread`");
        };
        match crate::github::pr_resolve_thread(thread).await {
            Ok(()) => Response::ok(id, 0, "resolved".to_string(), String::new()),
            Err(e) => Response::err(id, format!("resolve_thread: {e}")),
        }
    }
}
