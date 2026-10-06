//! The `context_*` RPC ops: how an agent reads the project context and writes
//! what it learned or decided back. Wraps whichever dispatcher the spawn chose
//! (git, roadmap, workflow comms) and delegates everything else to it, so the
//! layer is one wrapper regardless of the agent's purpose.
//!
//! Every write is stamped here, never from `args`: the agent and provider from
//! the spawn, the session as the source reference, and the branch and commit
//! of the checkout at call time (best effort) as provenance — which is what
//! later turns an assertion from provisional to confirmed or abandoned.
//!
//! A decision that collides with a current head is not written on the first
//! call; the reply names the head and the agent resubmits saying how the two
//! relate. Nothing here can retract, confirm or archive — those are rulings.

mod args;
mod read;
mod write;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::context::{self, render, Author, ContextStore, Provenance, Source, SourceKind, Stamp};
use crate::host::EngineCtx;
use crate::roadmap::Db;
use crate::rpc::{Response, RpcDispatcher, RpcEvent, RpcFuture};

/// Pinned by a test against the instruction block so the two can't drift.
pub const OPS: [&str; 4] = [
    "context_get",
    "context_record_entity",
    "context_record_decision",
    "context_link",
];

/// Characters the spawn-time index may take in the instruction block.
const INDEX_CHARS: usize = 1500;

/// The whole namespace, so a typo'd op gets an error naming the real ones.
fn is_context_op(op: &str) -> bool {
    op.starts_with("context_")
}

pub struct ContextDispatcher {
    inner: Arc<dyn RpcDispatcher>,
    store: ContextStore,
    /// The project's context id (`context::context_project_id`), never the
    /// host-local `projects.id`.
    project_id: String,
    /// The host-local `projects.id`, for the roadmap brief the vision falls
    /// back to.
    fletch_project_id: String,
    agent_id: String,
    provider: String,
    /// The checkout the branch and commit are read from at call time.
    cwd: PathBuf,
    session_id: Option<String>,
    db: Db,
}

/// Wraps `inner` in a [`ContextDispatcher`] when the layer is on for the
/// project. Off, or anything failing on the way to the store, leaves `inner`
/// as it was: an agent without the ops is worth more than a spawn that fails,
/// and the instruction block is gated on the same path (see [`spawn_index`]).
pub fn wrap(
    ctx: &EngineCtx,
    inner: Arc<dyn RpcDispatcher>,
    fletch_project_id: &str,
    agent_id: &str,
    provider: &str,
    cwd: &Path,
    session_id: Option<&str>,
) -> Arc<dyn RpcDispatcher> {
    let Some(project_id) = project_context_id(ctx, fletch_project_id) else {
        return inner;
    };
    let store = match ctx.context() {
        Ok(store) => store.clone(),
        Err(e) => {
            tracing::warn!(
                "context: store unavailable, agent {agent_id} runs without the ops: {e}"
            );
            return inner;
        }
    };
    Arc::new(ContextDispatcher {
        inner,
        store,
        project_id,
        fletch_project_id: fletch_project_id.to_string(),
        agent_id: agent_id.to_string(),
        provider: provider.to_string(),
        cwd: cwd.to_path_buf(),
        session_id: session_id.map(str::to_string),
        db: ctx.db.clone(),
    })
}

/// The index the instruction block carries at spawn: `None` when the layer is
/// off for the project (or unreachable), `Some` — possibly empty — when on.
pub fn spawn_index(ctx: &EngineCtx, fletch_project_id: &str) -> Option<String> {
    let project_id = project_context_id(ctx, fletch_project_id)?;
    let graph = ctx
        .context()
        .and_then(|store| store.load(&project_id).map_err(Into::into));
    match graph {
        Ok(graph) => Some(render::render_index(&graph, INDEX_CHARS)),
        Err(e) => {
            tracing::warn!("context: index unavailable for project {fletch_project_id}: {e}");
            None
        }
    }
}

/// The project's context id when the layer is on for it. Holds the connection
/// only for the two settings reads, never across a store call.
fn project_context_id(ctx: &EngineCtx, fletch_project_id: &str) -> Option<String> {
    let conn = ctx.db.lock();
    if !context::enabled(&conn, fletch_project_id) {
        return None;
    }
    match context::context_project_id(&conn, fletch_project_id) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("context: no context id for project {fletch_project_id}: {e}");
            None
        }
    }
}

impl ContextDispatcher {
    /// The stamp for one write. Branch and commit are read now rather than at
    /// spawn because the agent moves the checkout as it works.
    async fn stamp(&self, stated_by_user: bool) -> Stamp {
        let kind = if stated_by_user {
            SourceKind::UserTurn
        } else {
            SourceKind::AgentTurn
        };
        let branch = rev_parse(&self.cwd, &["--abbrev-ref", "HEAD"])
            .await
            .filter(|b| b != "HEAD");
        let commit_sha = rev_parse(&self.cwd, &["HEAD"]).await;
        Stamp {
            author: Author::agent(&self.agent_id, &self.provider),
            source: Source::new(kind, self.session_id.clone()),
            provenance: Provenance {
                workspace_id: Some(self.agent_id.clone()),
                branch,
                commit_sha,
                session_id: self.session_id.clone(),
                turn_id: None,
            },
        }
    }
}

/// `git rev-parse <args>` in `cwd`; `None` on any failure (no checkout, no
/// commit yet, git missing) — provenance is best effort.
async fn rev_parse(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = crate::git_dist::command(cwd)
        .arg("rev-parse")
        .args(args)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

impl RpcDispatcher for ContextDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        Box::pin(async move {
            if !is_context_op(op) {
                return self.inner.dispatch(id, op, args).await;
            }
            let resp = match op {
                "context_get" => self.get(id, args),
                "context_record_entity" => self.record_entity(id, args).await,
                "context_record_decision" => self.record_decision(id, args).await,
                "context_link" => self.link(id, args).await,
                other => Response::err(
                    id,
                    format!(
                        "unknown context op: {other} — this session has {}",
                        OPS.join(", ")
                    ),
                ),
            };
            (resp, Vec::new())
        })
    }
}

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;

#[cfg(test)]
#[path = "tests/dispatcher.rs"]
mod dispatcher_tests;

#[cfg(test)]
#[path = "tests/ops.rs"]
mod ops_tests;

#[cfg(test)]
#[path = "tests/instructions.rs"]
mod instruction_tests;
