//! Deterministic ingestion from pull requests, and the lifecycle hooks that
//! settle a checkout's provisional assertions. No model is involved: the
//! agent already wrote a `## Decisions` section into its PR body (see
//! `instructions/git_actions.md`), so a merge is the moment those lines are
//! known to be true of the main branch and get recorded — and the moment
//! everything that checkout recorded while its branch was open stops being
//! provisional. A checkout archived without its PR merging is the opposite
//! signal: its provisional assertions are abandoned. Branch drift never has
//! to be detected; each checkout's fate decides, repo by repo in a
//! multi-repo workspace.
//!
//! Both hooks run on the supervisor's path (the PR watcher's tick, archive),
//! so they are best-effort: every failure is logged and none reaches the
//! caller.

pub mod pr;
pub mod structure;

pub use pr::{parse_decisions, ParsedDecision};
pub use structure::StructureDelta;

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::context::model::*;
use crate::context::service::{ContextService, Project};
use crate::context::{resolve, ContextError, Result};
use crate::github::PrFileChange;
use crate::host::EngineCtx;
use crate::roadmap::drainer::primary_repo_path;
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
    /// The modules the merge added, removed or moved.
    pub structure: StructureDelta,
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
    let Some((service, record, project)) = target(ctx, workspace_id) else {
        return;
    };
    let repo = match subdir {
        Some(subdir) => record.repos.iter().position(|r| r.subdir == subdir),
        None => (!record.repos.is_empty()).then_some(0),
    };
    let Some(index) = repo else {
        tracing::warn!(
            workspace_id,
            ?subdir,
            "context ingest: merged PR's checkout is not tracked"
        );
        return;
    };
    let repo = &record.repos[index];
    ingest_by_number(
        service,
        &project,
        workspace_id,
        repo,
        index == 0,
        pr.number,
        pr_reference(Some(&pr.url), &repo.subdir, pr.number),
        pr.branch.clone(),
    )
    .await;
}

/// The archive entry: every checkout is settled by its own fate. A repo
/// whose PR merged while the watcher was not looking (the host was down,
/// say) is ingested first — idempotent through the per-reference check in
/// [`ingest_merged`], so a PR the watcher already handled costs one local
/// query — and then settled; one without a merge has its provisionals
/// abandoned.
pub async fn on_workspace_archived(ctx: &EngineCtx, workspace_id: &str) {
    let Some((service, record, project)) = target(ctx, workspace_id) else {
        return;
    };
    if record.repos.is_empty() {
        // No checkout, no PR: whatever this workspace recorded (stamped with
        // no repo) was never going to merge. The empty repo name matches no
        // stamped record; `is_primary` catches the unstamped ones.
        let stamp = stamp(workspace_id, "", None, None, None);
        if let Err(e) = service.settle(&project, workspace_id, "", true, false, stamp) {
            tracing::warn!(workspace_id, error = %e, "context ingest: archived workspace not settled");
        }
    }
    for (index, repo) in record.repos.iter().enumerate() {
        let is_primary = index == 0;
        let merged = repo.pr_state.as_deref() == Some("merged");
        if let (true, Some(number)) = (merged, repo.pr_number.and_then(|n| u32::try_from(n).ok())) {
            ingest_by_number(
                service,
                &project,
                workspace_id,
                repo,
                is_primary,
                number,
                pr_reference(repo.pr_url.as_deref(), &repo.subdir, number),
                repo.branch.clone(),
            )
            .await;
        }
        let stamp = stamp(workspace_id, &repo.subdir, None, None, None);
        match service.settle(
            &project,
            workspace_id,
            &repo.subdir,
            is_primary,
            merged,
            stamp,
        ) {
            Ok(0) => {}
            Ok(settled) => tracing::info!(
                workspace_id,
                subdir = %repo.subdir,
                merged,
                settled,
                "context ingest: archived checkout settled"
            ),
            Err(e) => {
                tracing::warn!(workspace_id, subdir = %repo.subdir, merged, error = %e, "context ingest: archived checkout not settled")
            }
        }
    }
}

/// What every entry point resolves once and passes down: the service, the
/// workspace record and its project through the gate. `None` when the
/// record is missing (logged) or the layer is off for the project (silent).
fn target<'a>(
    ctx: &'a EngineCtx,
    workspace_id: &str,
) -> Option<(&'a ContextService, AgentRecord, Project)> {
    let record = match WorkspaceManager::new(ctx.db.clone()).agent(workspace_id) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!(workspace_id, error = %e, "context ingest: no workspace record");
            return None;
        }
    };
    let service = match ctx.context() {
        Ok(service) => service,
        Err(e) => {
            tracing::warn!(workspace_id, error = %e, "context ingest: no context service");
            return None;
        }
    };
    match service.open(&record.project_id) {
        Ok(project) => Some((service, record, project)),
        Err(ContextError::Disabled) => None,
        Err(e) => {
            tracing::warn!(workspace_id, error = %e, "context ingest: no context project");
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

/// Fetch the PR body, merge commit and, when the source repo is the
/// project's primary one, changed files through the GitHub client, then read
/// the structural delta and ingest. The source repo backs the slug lookup
/// when the checkout is already gone. Any failure is logged and leaves the
/// PR unseen, for the next announcement to retry.
#[allow(clippy::too_many_arguments)]
async fn ingest_by_number(
    service: &ContextService,
    project: &Project,
    workspace_id: &str,
    repo: &TrackedRepo,
    is_primary: bool,
    number: u32,
    reference: String,
    branch: Option<String>,
) {
    if matches!(seen(service, &project.id, &reference), Ok(true)) {
        return;
    }
    let Ok(checkout) = repo.checkout_path(workspace_id) else {
        return;
    };
    let merge = match crate::github::pr_body_and_merge(&checkout, Some(&repo.repo_path), number)
        .await
    {
        Ok(Some(merge)) => merge,
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
    // Modules are the project's primary repo's, the one the bootstrap maps:
    // another repo's tree would collide with it on slugs such as `src`.
    let mapped = service
        .with_conn(|conn| primary_repo_path(conn, &project.fletch_id))
        .is_some_and(|primary| Path::new(&primary) == repo.repo_path);
    let changes = if mapped && merge.merge_sha.is_some() {
        match crate::github::pr_files(&checkout, Some(&repo.repo_path), number).await {
            Ok(Some(changes)) => Some(changes),
            Ok(None) => {
                tracing::info!(
                    workspace_id,
                    pr = number,
                    "context ingest: merged PR files unavailable"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(workspace_id, pr = number, error = %e, "context ingest: merged PR files fetch failed");
                return;
            }
        }
    } else {
        None
    };
    let pr = MergedPr {
        reference,
        body: merge.body,
        branch: branch.or_else(|| repo.branch.clone()),
        sha: merge.merge_sha,
        structure: StructureDelta::default(),
    };
    let source = changes.as_deref().map(|c| (repo.repo_path.as_path(), c));
    if let Err(e) = read_and_ingest(
        service,
        project,
        workspace_id,
        &repo.subdir,
        is_primary,
        pr,
        source,
    )
    .await
    {
        tracing::warn!(workspace_id, pr = number, error = %e, "context ingest: merged PR not ingested");
    }
}

/// Read the merge's structural delta from `source` (the primary source repo
/// and the files the PR changed), then [`ingest_merged`]. A read that fails
/// fails the ingest before anything marks the PR as seen, so the next
/// announcement of the merge (the watcher's tick, the archive) retries it.
async fn read_and_ingest(
    service: &ContextService,
    project: &Project,
    workspace_id: &str,
    repo: &str,
    is_primary: bool,
    mut pr: MergedPr,
    source: Option<(&Path, &[PrFileChange])>,
) -> crate::error::Result<()> {
    if let (Some((source_repo, changes)), Some(sha)) = (source, pr.sha.as_deref()) {
        pr.structure = structure::read(source_repo, sha, changes).await?;
    }
    ingest_merged(service, project, workspace_id, repo, is_primary, &pr)?;
    Ok(())
}

/// The store-level work of a merge, for tests and for any caller that
/// already resolved the project: land the PR's structural delta, record its
/// `## Decisions` under the write policy, then settle the checkout (`repo`,
/// the subdir; `is_primary` whether it is the workspace's first repo, whose
/// fate also covers records stamped with no repo). Structure goes first so a
/// decision line can name a module the same PR added.
///
/// The observation that marks the PR as ingested is recorded last, once every
/// line has landed and the checkout is settled, so a failure part-way leaves
/// the PR unseen and the next announcement of the same merge (the archive
/// path, a restart) finishes the job: the structure and the lines that did
/// land are already there and are skipped.
pub fn ingest_merged(
    service: &ContextService,
    project: &Project,
    workspace_id: &str,
    repo: &str,
    is_primary: bool,
    pr: &MergedPr,
) -> Result<()> {
    if seen(service, &project.id, &pr.reference)? {
        tracing::debug!(project = %project.id, pr = %pr.reference, "context ingest: PR already ingested");
        return Ok(());
    }
    let stamp = stamp(
        workspace_id,
        repo,
        Some(pr.reference.clone()),
        pr.branch.clone(),
        pr.sha.clone(),
    );
    structure::apply(service, project, &pr.structure, &stamp)?;
    let graph = service.store().load(&project.id)?;
    for decision in parse_decisions(&pr.body) {
        land(service, project, &graph, &stamp, decision)?;
    }
    service.settle(project, workspace_id, repo, is_primary, true, stamp.clone())?;
    let now = chrono::Utc::now().timestamp_millis();
    service.store().add_observation(&Observation {
        id: new_id(),
        project_id: project.id.clone(),
        source: stamp.source,
        provenance: stamp.provenance,
        input_hash: format!("{:x}", Sha256::digest(pr.body.as_bytes())),
        plan: None,
        created_at: now,
        extracted_at: Some(now),
    })?;
    Ok(())
}

/// Hand one decision line to the write policy as a confirmed candidate with
/// no relation of its own: whether it replaces or contradicts a head is a
/// judgment, and two decisions about one entity are usually both true. The
/// store lands it, holds it or finds it a duplicate.
fn land(
    service: &ContextService,
    project: &Project,
    graph: &Graph,
    stamp: &Stamp,
    decision: ParsedDecision,
) -> Result<()> {
    let (found, unknown) = resolve::entities(graph, &decision.about);
    if found.is_empty() || !unknown.is_empty() {
        tracing::warn!(
            project = %project.id,
            line = %decision.line,
            ?unknown,
            "context ingest: decision line skipped — unknown or missing entities"
        );
        return Ok(());
    }
    let candidate = Candidate {
        input: AssertionInput {
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
        },
        user: None,
        relation: None,
        evidence: vec![Evidence {
            session_id: None,
            turn_id: None,
            quote: decision.line.clone(),
        }],
        about_pending: Vec::new(),
        observation_id: None,
    };
    match service.record_decision(project, candidate, stamp.clone())? {
        Landing::Recorded { .. } => {}
        Landing::Held { .. } => {
            tracing::info!(project = %project.id, line = %decision.line, "context ingest: decision held for review")
        }
        Landing::Dismissed { .. } => {
            tracing::debug!(project = %project.id, line = %decision.line, "context ingest: decision dismissed before; skipped")
        }
        Landing::Duplicate { .. } => {
            tracing::debug!(project = %project.id, line = %decision.line, "context ingest: duplicate decision skipped")
        }
        Landing::Related { .. } => {
            tracing::warn!(project = %project.id, line = %decision.line, "context ingest: the write policy asked the ingester to relate; line skipped")
        }
    }
    Ok(())
}

/// Whether a PR observation with this reference already exists for the
/// project — the merge hook's idempotence key, since the watcher, the archive
/// path and a restart can each announce one merge.
fn seen(service: &ContextService, project_id: &str, reference: &str) -> Result<bool> {
    let count: i64 = service.with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM context.observations
              WHERE project_id = ?1
                AND json_extract(source, '$.kind') = 'pr'
                AND json_extract(source, '$.reference') = ?2",
            [project_id, reference],
            |r| r.get(0),
        )
    })?;
    Ok(count > 0)
}

fn stamp(
    workspace_id: &str,
    repo: &str,
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
            repo: Some(repo.to_string()),
            ..Default::default()
        },
    }
}

#[cfg(test)]
#[path = "tests/ingest.rs"]
mod tests;
