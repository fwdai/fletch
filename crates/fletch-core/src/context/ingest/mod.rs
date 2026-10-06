//! Deterministic ingestion from pull requests, and the lifecycle hooks that
//! settle a workspace's provisional assertions. No model is involved: the
//! agent already wrote a `## Decisions` section into its PR body (see
//! `instructions/git_actions.md`), so a merge is the moment those lines are
//! known to be true of the main branch and get recorded as *confirmed* —
//! and the moment everything the workspace recorded while its branch was
//! open stops being provisional. A workspace archived without its PR merging
//! is the opposite signal: its provisional assertions are abandoned. Branch
//! drift never has to be detected; the branch's fate decides.
//!
//! Both hooks run on the supervisor's path (the PR watcher's tick, archive),
//! so they are best-effort: every failure is logged and none reaches the
//! caller.

pub mod pr;

pub use pr::{parse_decisions, ParsedDecision};

use sha2::{Digest, Sha256};

use super::model::*;
use super::{resolve, ContextStore, Result};
use crate::host::EngineCtx;
use crate::workspace::{AgentRecord, TrackedRepo, WorkspaceManager};

/// What the host knows about a PR it just saw merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPr {
    /// The PR's URL, or `<subdir>#<number>` when that is all there is — see
    /// [`pr_reference`].
    pub reference: String,
    pub body: String,
    pub branch: Option<String>,
    /// Merge or head commit, when known.
    pub sha: Option<String>,
}

/// The PR watcher's entry: it has the PR's state and checkout, not its body.
/// Fetches the body through the GitHub client and ingests it. Nothing is
/// fetched for a project with the layer off.
pub async fn on_pr_merged_by_number(
    ctx: &EngineCtx,
    workspace_id: &str,
    subdir: Option<&str>,
    pr: &crate::github::PrState,
) {
    let Some((record, project_id)) = target(ctx, workspace_id) else {
        return;
    };
    let repo = match subdir {
        Some(subdir) => record.repos.iter().find(|r| r.subdir == subdir),
        None => record.repos.first(),
    };
    let Some(repo) = repo else {
        tracing::warn!(
            workspace_id,
            ?subdir,
            "context ingest: merged PR's checkout is not tracked"
        );
        return;
    };
    ingest_by_number(
        ctx,
        &project_id,
        workspace_id,
        repo,
        pr.number,
        pr_reference(Some(&pr.url), &repo.subdir, pr.number),
        pr.branch.clone(),
    )
    .await;
}

/// The archive entry for a workspace whose PR merged while the watcher was
/// not looking (the host was down, say): every merged repo's PR is fetched
/// and ingested. Idempotent through the per-reference check in
/// [`ingest_merged`], so a PR the watcher already handled costs one read.
pub async fn on_archive_merged(ctx: &EngineCtx, workspace_id: &str) {
    let Some((record, project_id)) = target(ctx, workspace_id) else {
        return;
    };
    for repo in &record.repos {
        let (Some("merged"), Some(number)) = (repo.pr_state.as_deref(), repo.pr_number) else {
            continue;
        };
        let Ok(number) = u32::try_from(number) else {
            continue;
        };
        ingest_by_number(
            ctx,
            &project_id,
            workspace_id,
            repo,
            number,
            pr_reference(repo.pr_url.as_deref(), &repo.subdir, number),
            repo.branch.clone(),
        )
        .await;
    }
}

/// The workspace's PR merged: record its `## Decisions` as confirmed
/// assertions and confirm whatever the workspace recorded provisionally.
/// Gated on the project's `context.enabled`; a second call for the same PR
/// reference is a no-op.
pub fn on_pr_merged(ctx: &EngineCtx, workspace_id: &str, pr: &MergedPr) {
    if let Some((_, project_id)) = target(ctx, workspace_id) {
        ingest(ctx, &project_id, workspace_id, pr);
    }
}

/// The workspace was archived. Without a merge, its provisional assertions
/// are abandoned; with one, they are confirmed in case the merge hook never
/// ran (the PR settled while the host was down, say). Confirming twice is
/// harmless: a confirmed assertion is no longer provisional.
pub fn on_workspace_archived(ctx: &EngineCtx, workspace_id: &str, merged: bool) {
    let Some((_, project_id)) = target(ctx, workspace_id) else {
        return;
    };
    let outcome = (|| -> crate::error::Result<usize> {
        Ok(settle_workspace(
            ctx.context()?,
            &project_id,
            workspace_id,
            merged,
            &stamp(workspace_id, None, None, None),
        )?)
    })();
    match outcome {
        Ok(0) => {}
        Ok(settled) => tracing::info!(
            workspace_id,
            merged,
            settled,
            "context ingest: archived workspace settled"
        ),
        Err(e) => {
            tracing::warn!(workspace_id, merged, error = %e, "context ingest: archived workspace not settled")
        }
    }
}

/// What every entry point resolves once and passes down: the workspace record
/// and the context project id of its project. `None` when the record is
/// missing (logged) or the layer is off for the project (silent).
fn target(ctx: &EngineCtx, workspace_id: &str) -> Option<(AgentRecord, String)> {
    let record = match WorkspaceManager::new(ctx.db.clone()).agent(workspace_id) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!(workspace_id, error = %e, "context ingest: no workspace record");
            return None;
        }
    };
    let conn = ctx.db.lock();
    if !super::enabled(&conn, &record.project_id) {
        return None;
    }
    match super::context_project_id(&conn, &record.project_id) {
        Ok(project_id) => Some((record, project_id)),
        Err(e) => {
            tracing::warn!(workspace_id, error = %e, "context ingest: no context project id");
            None
        }
    }
}

/// The idempotence key of a merged PR, computed the same way on the watcher's
/// path and the archive's: its URL when known, else `<subdir>#<number>` — the
/// number alone collides between the repos of a multi-repo project.
fn pr_reference(url: Option<&str>, subdir: &str, number: u32) -> String {
    match url.filter(|u| !u.is_empty()) {
        Some(url) => url.to_string(),
        None => format!("{subdir}#{number}"),
    }
}

/// Fetch the PR body through the GitHub client and ingest it. The source repo
/// backs the slug lookup when the checkout is already gone.
async fn ingest_by_number(
    ctx: &EngineCtx,
    project_id: &str,
    workspace_id: &str,
    repo: &TrackedRepo,
    number: u32,
    reference: String,
    branch: Option<String>,
) {
    let Ok(checkout) = repo.checkout_path(workspace_id) else {
        return;
    };
    let body = match crate::github::pr_body(&checkout, Some(&repo.repo_path), number).await {
        Ok(Some(body)) => body,
        Ok(None) => {
            tracing::info!(
                workspace_id,
                pr = number,
                "context ingest: merged PR body unavailable"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(workspace_id, pr = number, error = %e, "context ingest: merged PR body fetch failed");
            return;
        }
    };
    ingest(
        ctx,
        project_id,
        workspace_id,
        &MergedPr {
            reference,
            body,
            branch: branch.or_else(|| repo.branch.clone()),
            sha: None,
        },
    );
}

/// [`ingest_merged`] on the host's store, with the failure logged.
fn ingest(ctx: &EngineCtx, project_id: &str, workspace_id: &str, pr: &MergedPr) {
    let outcome = (|| -> crate::error::Result<()> {
        Ok(ingest_merged(ctx.context()?, project_id, workspace_id, pr)?)
    })();
    if let Err(e) = outcome {
        tracing::warn!(workspace_id, pr = %pr.reference, error = %e, "context ingest: merged PR not ingested");
    }
}

/// [`on_pr_merged`] below the host: the store-level work, for tests and for
/// any caller that already resolved the project.
///
/// The observation that marks the PR as ingested is recorded last, once every
/// line has landed and the workspace's provisionals are settled, so a failure
/// part-way leaves the PR unseen and the next announcement of the same merge
/// (the archive path, a restart) finishes the job: the lines that did land
/// classify as duplicates and are skipped.
pub fn ingest_merged(
    store: &ContextStore,
    project_id: &str,
    workspace_id: &str,
    pr: &MergedPr,
) -> Result<()> {
    if seen(store, project_id, &pr.reference)? {
        tracing::debug!(project_id, pr = %pr.reference, "context ingest: PR already ingested");
        return Ok(());
    }
    let stamp = stamp(
        workspace_id,
        Some(pr.reference.clone()),
        pr.branch.clone(),
        pr.sha.clone(),
    );
    for decision in parse_decisions(&pr.body) {
        land(store, project_id, &stamp, decision)?;
    }
    settle_workspace(store, project_id, workspace_id, true, &stamp)?;
    let now = chrono::Utc::now().timestamp_millis();
    store.add_observation(&Observation {
        id: new_id(),
        project_id: project_id.to_string(),
        source: stamp.source,
        provenance: stamp.provenance,
        input_hash: format!("{:x}", Sha256::digest(pr.body.as_bytes())),
        plan: None,
        created_at: now,
        extracted_at: Some(now),
    })?;
    Ok(())
}

/// Record one decision line as a confirmed head. A line that restates a live
/// head (same kind, domain, stance and statement) is skipped; one that says
/// something different lands next to the heads about the same entities —
/// whether it replaces or contradicts them is a judgment, and two decisions
/// about one entity are usually both true. The graph is reloaded per line
/// because each landed line changes what the next one is checked against.
fn land(
    store: &ContextStore,
    project_id: &str,
    stamp: &Stamp,
    decision: ParsedDecision,
) -> Result<()> {
    let graph = store.load(project_id)?;
    let (found, unknown) = resolve::entities(&graph, &decision.about);
    if found.is_empty() || !unknown.is_empty() {
        tracing::warn!(
            project_id,
            line = %decision.line,
            ?unknown,
            "context ingest: decision line skipped — unknown or missing entities"
        );
        return Ok(());
    }
    let input = AssertionInput {
        kind: decision.kind,
        domain: decision.domain,
        stance: decision.stance,
        statement: decision.statement,
        rationale: decision.rationale,
        valid_from: None,
        paths: Vec::new(),
        about: found.iter().map(|e| e.id.clone()).collect(),
        supersedes: None,
        contradicts: Vec::new(),
        status: AssertionStatus::Confirmed,
    };
    if resolve::classify(&graph, &input).kind == RelationKind::Duplicate {
        tracing::debug!(project_id, line = %decision.line, "context ingest: duplicate decision skipped");
        return Ok(());
    }
    store.record_assertion(project_id, input, stamp.clone())?;
    Ok(())
}

/// Confirm (`merged`) or abandon every provisional assertion the workspace
/// recorded. Returns how many were settled.
pub fn settle_workspace(
    store: &ContextStore,
    project_id: &str,
    workspace_id: &str,
    merged: bool,
    stamp: &Stamp,
) -> Result<usize> {
    let ids = store.provisional_for_workspace(project_id, workspace_id)?;
    for id in &ids {
        if merged {
            store.confirm(project_id, id, stamp.clone())?;
        } else {
            store.abandon(project_id, id, stamp.clone())?;
        }
    }
    Ok(ids.len())
}

/// Whether a PR observation with this reference already exists for the
/// project — the merge hook's idempotence key, since the watcher, the archive
/// path and a restart can each announce one merge.
fn seen(store: &ContextStore, project_id: &str, reference: &str) -> Result<bool> {
    let conn = store.db().lock();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM context.observations
          WHERE project_id = ?1
            AND json_extract(source, '$.kind') = 'pr'
            AND json_extract(source, '$.reference') = ?2",
        [project_id, reference],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn stamp(
    workspace_id: &str,
    reference: Option<String>,
    branch: Option<String>,
    sha: Option<String>,
) -> Stamp {
    Stamp {
        author: Author::ingester(),
        source: Source::new(SourceKind::Pr, reference),
        provenance: Provenance {
            workspace_id: Some(workspace_id.to_string()),
            branch,
            commit_sha: sha,
            ..Default::default()
        },
    }
}

#[cfg(test)]
#[path = "tests/ingest.rs"]
mod tests;
