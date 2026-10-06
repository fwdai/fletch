//! The `context_*` RPC ops: how an agent reads the project context and writes
//! what it learned or decided back. Wraps whichever dispatcher the spawn chose
//! (git, roadmap, workflow comms) and delegates everything else to it, so the
//! layer is one wrapper regardless of the agent's purpose.
//!
//! Every write is stamped here, never from `args`: the agent and provider from
//! the spawn, the session as the source reference, and the primary checkout
//! plus its branch and commit at call time (best effort) as provenance —
//! which is what later turns an assertion from provisional to confirmed or
//! abandoned.
//!
//! Every write goes through [`ContextService`]; the policy for what lands is
//! `ContextStore::land`'s ([`context::Landing`]), and this module only maps
//! its outcome onto the wire. A decision that sits next to current heads is
//! not written on the first call; the reply names the heads and the agent
//! resubmits saying how they relate. Nothing here can retract, confirm or
//! archive — those are rulings.

mod args;
mod read;
mod write;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::context::{self, render, Author, ContextService, Provenance, Source, SourceKind, Stamp};
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
    service: ContextService,
    /// The project as the gate let it through at spawn.
    project: context::Project,
    agent_id: String,
    provider: String,
    /// The checkout the branch and commit are read from at call time.
    cwd: PathBuf,
    /// The workspace's checkouts (repo subdirs), primary first. A write is
    /// stamped with the one it is about — the unit a merge or an archive
    /// settles — so a workspace with several must be told which.
    repos: Vec<String>,
    session_id: Option<String>,
    db: Db,
}

/// Wraps `inner` in a [`ContextDispatcher`] when the layer is on for the
/// project. Off, or anything failing on the way to the service, leaves
/// `inner` as it was: an agent without the ops is worth more than a spawn
/// that fails, and the instruction block is gated on the same path (see
/// [`spawn_index`]). `repo` is the workspace's primary checkout subdir.
#[allow(clippy::too_many_arguments)]
pub fn wrap(
    ctx: &EngineCtx,
    inner: Arc<dyn RpcDispatcher>,
    fletch_project_id: &str,
    agent_id: &str,
    provider: &str,
    cwd: &Path,
    repos: Vec<String>,
    session_id: Option<&str>,
) -> Arc<dyn RpcDispatcher> {
    let Some((service, project)) = open(ctx, fletch_project_id) else {
        return inner;
    };
    Arc::new(ContextDispatcher {
        inner,
        service,
        project,
        agent_id: agent_id.to_string(),
        provider: provider.to_string(),
        cwd: cwd.to_path_buf(),
        repos,
        session_id: session_id.map(str::to_string),
        db: ctx.db.clone(),
    })
}

/// The index the instruction block carries at spawn: `None` when the layer is
/// off for the project (or unreachable), `Some` — possibly empty — when on.
pub fn spawn_index(ctx: &EngineCtx, fletch_project_id: &str) -> Option<String> {
    let (service, project) = open(ctx, fletch_project_id)?;
    match service.store().load(&project.id) {
        Ok(graph) => Some(render::render_index(&graph, INDEX_CHARS)),
        Err(e) => {
            tracing::warn!("context: index unavailable for project {fletch_project_id}: {e}");
            None
        }
    }
}

/// The service and the project through its gate; `None` when the layer is
/// off for the project (quietly) or unreachable (with a warning).
fn open(ctx: &EngineCtx, fletch_project_id: &str) -> Option<(ContextService, context::Project)> {
    let service = match ctx.context() {
        Ok(service) => service.clone(),
        Err(e) => {
            tracing::warn!("context: service unavailable for project {fletch_project_id}: {e}");
            return None;
        }
    };
    match service.open(fletch_project_id) {
        Ok(project) => Some((service, project)),
        Err(context::ContextError::Disabled) => None,
        Err(e) => {
            tracing::warn!("context: no context id for project {fletch_project_id}: {e}");
            None
        }
    }
}

impl ContextDispatcher {
    /// The checkout a write is about: the one named, or the only one. With
    /// several and none named the write is refused — a decision made while
    /// changing one repo must not be settled by another's merge.
    fn checkout(&self, named: Option<&str>) -> Result<Option<String>, String> {
        let named = named.map(str::trim).filter(|r| !r.is_empty());
        match (named, self.repos.as_slice()) {
            (Some(r), repos) if repos.iter().any(|known| known == r) => Ok(Some(r.to_string())),
            (Some(r), repos) => Err(format!(
                "`repo` must be one of this workspace's checkouts ({}), not `{r}`",
                repos.join(", ")
            )),
            (None, []) => Ok(None),
            (None, [only]) => Ok(Some(only.clone())),
            (None, repos) => Err(format!(
                "this workspace has several checkouts ({}); say which one with `repo`",
                repos.join(", ")
            )),
        }
    }

    /// The stamp for one write. Branch and commit are read now rather than at
    /// spawn because the agent moves the checkout as it works. The source is
    /// the agent's turn: a user quote is applied by the service, the only
    /// place that can turn one into a `user_turn` source.
    async fn stamp(&self, repo: Option<String>) -> Stamp {
        let source = Source::new(SourceKind::AgentTurn, self.session_id.clone());
        let branch = rev_parse(&self.cwd, &["--abbrev-ref", "HEAD"])
            .await
            .filter(|b| b != "HEAD");
        let commit_sha = rev_parse(&self.cwd, &["HEAD"]).await;
        Stamp {
            author: Author::agent(&self.agent_id, &self.provider),
            source,
            provenance: Provenance {
                workspace_id: Some(self.agent_id.clone()),
                branch,
                commit_sha,
                session_id: self.session_id.clone(),
                turn_id: None,
                repo,
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
