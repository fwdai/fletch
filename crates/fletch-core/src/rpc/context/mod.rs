//! The `context_*` RPC ops: how an agent reads the project context and writes
//! what it learned or decided back. Wraps whichever dispatcher the spawn chose
//! (git, roadmap, workflow comms) and delegates everything else to it, so the
//! layer is one wrapper regardless of the agent's purpose.
//!
//! Every write is stamped here, never from `args`: the agent and provider from
//! the spawn, the session as the source reference, and the selected checkout
//! plus its branch and commit at call time (best effort) as provenance. The
//! checkout is resolved live because repositories can be added while an agent
//! runs; its fate later turns the assertion provisional, confirmed or abandoned.
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
use crate::workspace::WorkspaceManager;

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
    /// The project as the gate let it through at spawn — a name, not a
    /// permission: every op asks the service again.
    project: context::Project,
    agent_id: String,
    provider: String,
    /// Resolves the workspace's checkouts at call time. Repositories can be
    /// added to a live agent, so spawn-time state is not authoritative.
    checkouts: CheckoutResolver,
    session_id: Option<String>,
    db: Db,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Checkout {
    repo: String,
    path: PathBuf,
}

type CheckoutResolver = Arc<dyn Fn() -> Result<Vec<Checkout>, String> + Send + Sync>;

/// Wraps `inner` in a [`ContextDispatcher`] when the layer is on for the
/// project. Off, or anything failing on the way to the service, leaves
/// `inner` as it was: an agent without the ops is worth more than a spawn
/// that fails, and the instruction block is gated on the same path (see
/// [`spawn_index`]). Checkout identity and paths are deliberately resolved from
/// the workspace record on every write: `add_repo_to_agent` can extend a live
/// agent without rebuilding its RPC dispatcher.
pub fn wrap(
    ctx: &EngineCtx,
    inner: Arc<dyn RpcDispatcher>,
    fletch_project_id: &str,
    agent_id: &str,
    provider: &str,
    session_id: Option<&str>,
) -> Arc<dyn RpcDispatcher> {
    let Some((service, project)) = open(ctx, fletch_project_id) else {
        return inner;
    };
    let workspace = WorkspaceManager::new(ctx.db.clone());
    let checkout_agent_id = agent_id.to_string();
    let checkouts: CheckoutResolver = Arc::new(move || {
        let record = workspace
            .agent(&checkout_agent_id)
            .map_err(|e| format!("could not read this workspace's checkouts: {e}"))?;
        record
            .repos
            .into_iter()
            .map(|repo| {
                let path = repo
                    .checkout_path(&checkout_agent_id)
                    .map_err(|e| format!("could not resolve checkout `{}`: {e}", repo.subdir))?;
                Ok(Checkout {
                    repo: repo.subdir,
                    path,
                })
            })
            .collect()
    });
    Arc::new(ContextDispatcher {
        inner,
        service,
        project,
        agent_id: agent_id.to_string(),
        provider: provider.to_string(),
        checkouts,
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
    fn checkout(&self, named: Option<&str>) -> Result<Checkout, String> {
        let named = named.map(str::trim).filter(|r| !r.is_empty());
        let checkouts = (self.checkouts)()?;
        match (named, checkouts.as_slice()) {
            (Some(r), repos) => repos
                .iter()
                .find(|known| known.repo == r)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "`repo` must be one of this workspace's checkouts ({}), not `{r}`",
                        repos
                            .iter()
                            .map(|checkout| checkout.repo.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
            (None, []) => Err("this workspace has no checkouts".into()),
            (None, [only]) => Ok(only.clone()),
            (None, repos) => Err(format!(
                "this workspace has several checkouts ({}); say which one with `repo`",
                repos
                    .iter()
                    .map(|checkout| checkout.repo.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    fn primary_checkout(&self) -> Option<Checkout> {
        (self.checkouts)().ok()?.into_iter().next()
    }

    /// The stamp for one write. Branch and commit are read now rather than at
    /// spawn because the agent moves the checkout as it works. The source is
    /// the agent's turn: a user quote is applied by the service, the only
    /// place that can turn one into a `user_turn` source.
    async fn stamp(&self, checkout: Option<&Checkout>, checkout_scoped: bool) -> Stamp {
        let source = Source::new(SourceKind::AgentTurn, self.session_id.clone());
        let branch = match checkout {
            Some(checkout) => rev_parse(&checkout.path, &["--abbrev-ref", "HEAD"])
                .await
                .filter(|b| b != "HEAD"),
            None => None,
        };
        let commit_sha = match checkout {
            Some(checkout) => rev_parse(&checkout.path, &["HEAD"]).await,
            None => None,
        };
        Stamp {
            author: Author::agent(&self.agent_id, &self.provider),
            source,
            provenance: Provenance {
                workspace_id: Some(self.agent_id.clone()),
                branch,
                commit_sha,
                session_id: self.session_id.clone(),
                turn_id: None,
                repo: checkout
                    .filter(|_| checkout_scoped)
                    .map(|checkout| checkout.repo.clone()),
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
            // The gate, now: the project was let through at spawn, and the
            // layer may have been turned off since. (A write reads it again
            // inside its own transaction; this is what refuses a read.)
            if let Err(e) = self.service.check(&self.project) {
                return (Response::err(id, e.to_string()), Vec::new());
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
