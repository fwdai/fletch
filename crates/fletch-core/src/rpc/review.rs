//! The `review_submit` RPC op for the local reviewer agent. Wraps
//! [`GitDispatcher`] and delegates everything else, so the reviewer keeps the
//! read-only git surface (its grant, `AgentCaps::reviewer`, refuses the rest).
//!
//! The reviewer is identified by the agent id stamped at construction, never
//! by `args`; the engine then finds the checkout whose review it is running.

use std::sync::Arc;

use serde_json::Value;

use crate::host::EngineCtx;
use crate::review::Finding;
use crate::rpc::git::GitDispatcher;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

pub const OP: &str = "review_submit";

pub struct ReviewDispatcher {
    ctx: Arc<EngineCtx>,
    agent_id: String,
    git: GitDispatcher,
}

impl ReviewDispatcher {
    pub fn new(ctx: Arc<EngineCtx>, agent_id: &str, git: GitDispatcher) -> Self {
        Self {
            ctx,
            agent_id: agent_id.to_string(),
            git,
        }
    }

    async fn submit(&self, id: &str, args: &Value) -> Response {
        let Some(verdict) = args.get("verdict").and_then(Value::as_str) else {
            return Response::err(
                id,
                "review_submit requires `verdict` (\"approve\" or \"revise\") and, for \
                 revise, `findings`",
            );
        };
        let findings: Vec<Finding> = match args.get("findings") {
            None | Some(Value::Null) => Vec::new(),
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(f) => f,
                Err(e) => {
                    return Response::err(
                        id,
                        format!(
                            "review_submit: `findings` must be a list of \
                             {{path, line, severity, title, body}}: {e}"
                        ),
                    )
                }
            },
        };
        if findings.iter().any(|f| f.title.trim().is_empty()) {
            return Response::err(id, "review_submit: every finding needs a `title`");
        }
        let Some(sup) = self.ctx.supervisor() else {
            return Response::err(id, "review_submit: the host is still starting");
        };
        match crate::review::engine::submit(&self.ctx, &sup, &self.agent_id, verdict, findings)
            .await
        {
            Ok(()) => Response::ok(id, 0, "submitted".to_string(), String::new()),
            Err(e) => Response::err(id, format!("review_submit: {e}")),
        }
    }
}

impl RpcDispatcher for ReviewDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            if op == OP {
                return (self.submit(id, args).await, Vec::new());
            }
            self.git.dispatch(id, op, args).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::sink::RecordingSink;
    use crate::rpc::caps::AgentCaps;
    use crate::supervisor::Supervisor;
    use crate::workspace::{new_agent_record, AgentView, TrackedRepo, WorkspaceManager};
    use serde_json::json;

    fn host(purpose: Option<&str>) -> (ReviewDispatcher, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let sink = Arc::new(RecordingSink::new());
        let ctx = Arc::new(EngineCtx::new(sink, db.clone(), Box::new(|| false)));
        let sup = Arc::new(Supervisor::new(Arc::new(WorkspaceManager::new(db))));
        ctx.set_supervisor(sup.clone());
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut record = new_agent_record(
            "rev".into(),
            "rev".into(),
            "codex".into(),
            TrackedRepo {
                repo_path: repo.clone(),
                subdir: "repo".into(),
                branch: None,
                parent_branch: None,
                base_sha: None,
                pr_number: None,
                pr_url: None,
                pr_title: None,
                pr_state: None,
                label: None,
                adopted_checkout: None,
            },
            String::new(),
            AgentView::Custom,
        );
        record.purpose = purpose.map(str::to_string);
        sup.workspace.add_agent(&mut record).unwrap();
        let git = GitDispatcher::new(repo.clone(), repo, "main".into(), AgentCaps::reviewer());
        (ReviewDispatcher::new(ctx, "rev", git), dir)
    }

    #[tokio::test]
    async fn the_args_are_validated_before_the_engine_is_asked() {
        let (disp, _dir) = host(Some(crate::workspace::PURPOSE_REVIEWER));
        let (resp, _) = disp.dispatch("1", OP, &json!({})).await;
        assert!(resp.error.unwrap().contains("requires `verdict`"));
        let (resp, _) = disp
            .dispatch("2", OP, &json!({ "verdict": "revise", "findings": "nope" }))
            .await;
        assert!(resp.error.unwrap().contains("must be a list"));
        let (resp, _) = disp
            .dispatch(
                "3",
                OP,
                &json!({ "verdict": "revise", "findings": [{ "title": " ", "body": "x" }] }),
            )
            .await;
        assert!(resp.error.unwrap().contains("needs a `title`"));
    }

    /// With nothing in flight the engine refuses, and so does a non-reviewer.
    #[tokio::test]
    async fn a_report_needs_a_review_in_flight_from_a_reviewer() {
        let (disp, _dir) = host(Some(crate::workspace::PURPOSE_REVIEWER));
        let (resp, _) = disp
            .dispatch("1", OP, &json!({ "verdict": "approve" }))
            .await;
        assert_eq!(
            resp.error.as_deref(),
            Some("review_submit: no review is in flight for this reviewer")
        );
        let (disp, _dir) = host(None);
        let (resp, _) = disp
            .dispatch("1", OP, &json!({ "verdict": "approve" }))
            .await;
        assert_eq!(
            resp.error.as_deref(),
            Some("review_submit: review_submit is for the reviewer agent only")
        );
    }

    /// Everything else is the git dispatcher's, under the reviewer's grant.
    #[tokio::test]
    async fn other_ops_fall_through_to_git_with_the_reviewer_grant() {
        let (disp, _dir) = host(Some(crate::workspace::PURPOSE_REVIEWER));
        let (resp, _) = disp.dispatch("1", "ping", &json!({})).await;
        assert_eq!(resp.stdout.as_deref(), Some("pong"));
        let (resp, _) = disp.dispatch("2", "git_push", &json!({})).await;
        assert!(resp.error.unwrap().contains("review_submit"));
    }
}
